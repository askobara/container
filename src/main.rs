#[macro_use]
extern crate lazy_static;

use anyhow::Result;
use futures::{StreamExt, TryStreamExt};

use clap::{Command, CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Generator, Shell};
use k8s_openapi::api::core::v1 as corev1;
use kube::{
    api::{Api, LogParams},
    Client,
};

use console::style;
use regex::Regex;
use utils::*;

mod utils;

use bollard::Docker;
use bytes::{Bytes, BytesMut};
use futures::Stream;

const DOCKER_BUFFER_SIZE: usize = 8192;

lazy_static! {
    static ref SYMFONY_LOG_RE: Regex = Regex::new(
        r"(?x)^
        \[(?P<dt>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}.\d+[-\+]\d{2}:\d{2})\]
        (?:\s+(?P<level>\w+\.\w+):)
        (?:\s+\[(?P<class>[^\]]+)\]\s+)?
        (?P<msg>.+)
    $").unwrap();

    static ref NGINX_LOG_RE: Regex = Regex::new(
        r#"(?x)^
        "(?P<dt>\d{2}/\w{3}/\d{4}:\d{2}:\d{2}:\d{2}\s\+\d{4})"
        (?:\s+(?P<msg>.+))
    $"#).unwrap();

    static ref PROXY_LOG_RE: Regex = Regex::new( // nginx
        r"(?x)^
        (?P<ip>((25[0-5]|(2[0-4]|1\d|[1-9]|)\d)\.?\b){4})
        (?:\s+-)
        (?:\s+(?P<username>.+))
        (?:\s+\[?(?P<dt>\d{2}/\w{3}/\d{4}:\d{2}:\d{2}:\d{2}\s\+\d{4})\]?)
        (?:\s+(?P<msg>.+))
    $").unwrap();

    static ref PHPFPM_LOG_RE: Regex = Regex::new(
        r"(?x)^
        \[(?P<dt>\d{2}-\w{3}-\d{4}\s\d{2}:\d{2}:\d{2})\]
        (?:\s+(?P<lvl>\w+):)
        (?:\s+\[(?P<instance>.+)\])
        (?:\s+(?P<msg>.+))
    $").unwrap();

    static ref PHP_MESSAGE_LOG_RE: Regex = Regex::new(
        r"(?x)^
        NOTICE:\sPHP\smessage:
        (?:\s(?P<dt>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:.\d+)?[-\+]\d{2}:\d{2}))
        (?:\s\[(?P<lvl>\w+)\])
        (?:\s+(?P<msg>.+))
        $"
    ).unwrap();

    static ref FASTCGI_ERROR_LOG_RE: Regex = Regex::new("FastCGI sent in stderr").unwrap();
    static ref REPLACE: Regex = Regex::new(r"\s\s+").unwrap();
}

#[derive(Debug, Parser)]
#[command(name = "container", author, version, about, long_about = None)] // Read from `Cargo.toml`
struct Cli {
    // If provided, outputs the completion file for given shell
    #[arg(long = "generate", value_enum)]
    generator: Option<Shell>,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command()]
    Logs {
        #[arg(short, long)]
        local: bool,
        #[arg(short, long)]
        container: Option<String>,
        #[arg(short, long)]
        namespace: Option<String>,
    },
}

fn parse_log_str(str: &str) -> Result<()> {
    if let Some(cap) = SYMFONY_LOG_RE.captures(&str) {
        match &cap["level"] {
            "security.DEBUG" | "security.INFO" => {}
            _ => {
                // let mut msg = REPLACE.replace_all(&cap["msg"], " ").to_string();

                let dt = chrono::DateTime::parse_from_rfc3339(&cap["dt"])?;

                println!(
                    "[{timestamp}] {lvl}:{class} {msg}",
                    timestamp = style(format_datetime(&dt)).yellow(),
                    lvl = style(&cap["level"]).bold().bright().underlined(),
                    class = cap
                        .name("class")
                        .map(|class| format!(" [{}]", style(class.as_str()).magenta()))
                        .unwrap_or(Default::default()),
                    msg = find_and_color_json(&mut cap["msg"].to_string()),
                );
            }
        }
    } else if let Some(cap) = PHP_MESSAGE_LOG_RE.captures(&str) {
        println!(
            "[{}] {} {}",
            style(&cap["dt"]).yellow(),
            style(&cap["lvl"]).bright().bold(),
            &cap["msg"]
        );
    } else if NGINX_LOG_RE.is_match(&str) {
        println!("NGINX_LOG_RE::::{str}");
        // ignore
    } else if PROXY_LOG_RE.is_match(&str) {
        // ignore
    } else if FASTCGI_ERROR_LOG_RE.is_match(&str) {
        // ignore
    } else if PHPFPM_LOG_RE.is_match(&str) {
        // ignore
    } else if !str.is_empty() {
        println!("{}", find_and_color_json(&mut str.to_string()));
    }

    Ok(())
}

async fn parse_kube_stream(
    mut stream: impl Stream<Item = kube::Result<Bytes>> + std::marker::Unpin,
) -> Result<()> {
    let mut buffer = BytesMut::new();

    while let Some(line) = stream.try_next().await? {
        let mut it = line.split(|n| *n == b'\n').peekable();
        let is_last_chunk = line.last().map(|n| *n == b'\n').unwrap_or(false);

        while let Some(chunk) = it.next() {
            buffer.extend_from_slice(chunk);

            if (it.peek().is_some() || is_last_chunk) && !buffer.is_empty() {
                let str = String::from_utf8_lossy(&buffer);
                parse_log_str(&str)?;
                buffer.clear();
            }
        }
    }

    Ok(())
}

async fn parse_docker_stream(
    mut stream: impl Stream<Item = core::result::Result<bollard::container::LogOutput, bollard::errors::Error>>
        + std::marker::Unpin,
) -> Result<()> {
    let mut buffer = BytesMut::new();

    while let Some(line) = stream.try_next().await?.map(|logs| logs.into_bytes()) {
        let is_last = line.len() < DOCKER_BUFFER_SIZE;

        if let Some((_, chunk)) = line.split_last() {
            buffer.extend_from_slice(&chunk);

            if is_last && !buffer.is_empty() {
                for str in String::from_utf8_lossy(&buffer).split(|c| c == '\n') {
                    parse_log_str(&str)?;
                }
                buffer.clear();
            }
        }
    }

    Ok(())
}

fn print_completions<G: Generator>(gen: G, cmd: &mut Command) {
    generate(gen, cmd, cmd.get_name().to_string(), &mut std::io::stdout());
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    if let Some(generator) = cli.generator {
        let mut cmd = Cli::command();
        eprintln!("Generating completion file for {generator:?}...");
        print_completions(generator, &mut cmd);

        return Ok(());
    } else if let Some(command) = cli.command {
        match command {
            Commands::Logs {
                local,
                container,
                namespace,
            } => {
                if local {
                    let docker = Docker::connect_with_local_defaults()?;

                    let options = Some(bollard::container::ListContainersOptions::default());
                    let containers = docker.list_containers::<String>(options).await?;

                    let names = containers
                        .iter()
                        .flat_map(|c| c.names.as_deref().unwrap_or_default())
                        .map(|c| {
                            let name = if c.starts_with('/') {
                                c.split_at('/'.len_utf8()).1
                            } else {
                                &c
                            };

                            name.to_owned()
                        });

                    let container_name = select_one(names, container.as_deref())?;

                    let options = Some(bollard::container::LogsOptions {
                        stdout: true,
                        stderr: true,
                        tail: "500",
                        ..Default::default()
                    });
                    let logs = docker.logs(&container_name, options);

                    parse_docker_stream(logs.boxed()).await?;
                } else {
                    let client = Client::try_default().await?;
                    let all_ns: Api<corev1::Namespace> = Api::all(client.clone());
                    let ns = kube_select_one(&all_ns, namespace.as_deref()).await?;

                    let pods: Api<corev1::Pod> = Api::namespaced(client, &ns);
                    let pod = kube_select_one(&pods, container.as_deref()).await?;

                    let logs = pods
                        .log_stream(
                            &pod,
                            &LogParams {
                                since_seconds: Some(1 * 60 * 60),
                                // tail_lines: Some(500),
                                ..LogParams::default()
                            },
                        )
                        .await?;

                    parse_kube_stream(logs.boxed()).await?;
                }
            }
        }
    }

    Ok(())
}
