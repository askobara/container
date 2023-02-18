#[macro_use]
extern crate lazy_static;

use anyhow::Result;
use futures::{StreamExt, TryStreamExt};
use skim::prelude::*;

use clap::{Command, CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Generator, Shell};
use k8s_openapi::api::core::v1 as corev1;
use kube::{
    api::{Api, ListParams, LogParams, ResourceExt},
    Client,
};

use colored_json::{ColoredFormatter, CompactFormatter};
use console::style;
use regex::Regex;
use tracing::error;
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
        \s+
        (?P<level>\w+\.\w+):
        \s+
        (?:\[(?P<class>[^\]]+)\]\s+)?
        (?P<msg>.+)
    $"
    )
    .unwrap();
    static ref NGINX_LOG_RE: Regex = Regex::new(
        r#"(?x)^
        "(?P<dt>\d{2}/\w{3}/\d{4}:\d{2}:\d{2}:\d{2}\s\+\d{4})"
        \s+
        (?P<msg>.+)
    $"#
    )
    .unwrap();
    static ref PROXY_LOG_RE: Regex = Regex::new(
        r"(?x)^
        (?P<ip>((25[0-5]|(2[0-4]|1\d|[1-9]|)\d)\.?\b){4})
        \s+
        (?P<hz>.+)
        \s+
        (?P<dt>\d{2}/\w{3}/\d{4}:\d{2}:\d{2}:\d{2}\s\+\d{4})
        \s+
        (?P<msg>.+)
    $"
    )
    .unwrap();
    static ref PHPFPM_LOG_RE: Regex = Regex::new(
        r"(?x)^
        \[(?P<dt>\d{2}-\w{3}-\d{4}\s\d{2}:\d{2}:\d{2})\]
        \s+
        (?P<lvl>\w+):
        \s+
        \[(?P<instance>.+)\]
        \s+
        (?P<msg>.+)
    $"
    )
    .unwrap();
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

fn search_json(str: &str) -> Vec<std::ops::Range<usize>> {
    let mut vec: Vec<std::ops::Range<usize>> = vec![];
    let mut stack: Vec<usize> = Vec::new();
    let mut inside_str = false;

    for (i, c) in str.bytes().enumerate() {
        if c == b'{' && !inside_str {
            stack.push(i);
        } else if c == b'"' {
            inside_str = !inside_str;
        } else if c == b'}' && !inside_str {
            if let Some(start) = stack.pop() {
                if stack.is_empty() {
                    vec.push(std::ops::Range { start, end: i + 1 });
                }
            }
        }
    }

    vec
}

fn parse_bytes(str: &str) -> Result<()> {
    if let Some(cap) = SYMFONY_LOG_RE.captures(&str) {
        match &cap["level"] {
            "security.DEBUG" | "security.INFO" => {}
            _ => {
                // let mut msg = REPLACE.replace_all(&cap["msg"], " ").to_string();
                let msg = &mut cap["msg"].to_string();

                for range in search_json(&msg) {
                    let colored = serde_json::from_str(&msg[range.clone()]).and_then(|json| {
                        ColoredFormatter::new(CompactFormatter {}).to_colored_json_auto(&json)
                    });

                    match colored {
                        Ok(slice) => msg.replace_range(range, &slice),
                        Err(e) => error!("{:?}", e),
                    };
                }

                let dt = chrono::DateTime::parse_from_rfc3339(&cap["dt"])?;

                println!(
                    "[{timestamp}] {lvl}:{class} {msg}",
                    timestamp = style(format_datetime(&dt)).yellow(),
                    lvl = style(&cap["level"]).bold().bright().underlined(),
                    class = cap
                        .name("class")
                        .map(|class| format!(" [{}]", style(class.as_str()).magenta()))
                        .unwrap_or(Default::default())
                );
            }
        }
    } else if let Some(_cap) = NGINX_LOG_RE.captures(&str) {
        // println!(
        //     "[{}] {}",
        //     style(&cap["dt"]).yellow(),
        //     &cap["msg"]
        // );
    } else if let Some(_cap) = PROXY_LOG_RE.captures(&str) {
        // println!(
        //     "{} {} [{}] {}",
        //     &cap["ip"],
        //     &cap["hz"],
        //     style(&cap["dt"]).yellow(),
        //     &cap["msg"]
        // );
    } else if let Some(_cap) = PHPFPM_LOG_RE.captures(&str) {
        // println!(
        //     "[{}] {}: [{}] {}",
        //     style(&cap["dt"]).yellow(),
        //     style(&cap["lvl"]).bright().bold(),
        //     &cap["instance"],
        //     &cap["msg"]
        // );
    } else if !str.is_empty() {
        println!("------ {str:?}");
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
                parse_bytes(&str)?;
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
                    parse_bytes(&str)?;
                }
                buffer.clear();
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct InnerNamespace {
    value: String,
}

impl SkimItem for InnerNamespace {
    fn text(&self) -> Cow<str> {
        Cow::Borrowed(&self.value)
    }

    fn preview(&self, _context: PreviewContext) -> ItemPreview {
        ItemPreview::Text(format!("{self:#?}"))
    }
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

                    let options = Some(bollard::container::ListContainersOptions {
                        all: false,
                        ..Default::default()
                    });
                    let containers = docker.list_containers::<String>(options).await?;

                    let n: Vec<String> = containers
                        .iter()
                        .flat_map(|c| c.names.clone().unwrap_or(Vec::new()))
                        .collect();
                    let w = select_one(n, container.as_deref())?;

                    let options = Some(bollard::container::LogsOptions {
                        stdout: true,
                        stderr: true,
                        tail: "500",
                        ..Default::default()
                    });
                    let logs = docker.logs(&w[1..], options);
                    parse_docker_stream(logs).await?;
                } else {
                    let client = Client::try_default().await?;
                    let all_ns: Api<corev1::Namespace> = Api::all(client.clone());
                    let ns = all_ns
                        .list(&ListParams::default())
                        .await
                        .map_err(anyhow::Error::new)
                        .and_then(|result| {
                            let items = result.iter().map(|item| InnerNamespace {
                                value: item.name_any(),
                            });
                            select_one(items, namespace.as_deref())
                        })?;

                    let pods: Api<corev1::Pod> = Api::namespaced(client, &ns.value);

                    let pod = pods
                        .list(&ListParams::default())
                        .await
                        .map_err(anyhow::Error::new)
                        .and_then(|result| {
                            let items = result.iter().map(|item| InnerNamespace {
                                value: item.name_any(),
                            });
                            select_one(items, container.as_deref())
                        })?;

                    let logs = pods
                        .log_stream(
                            &pod.value,
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
