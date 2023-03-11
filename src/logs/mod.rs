mod kube;
mod docker;
mod parse;

use anyhow::Result;
use crate::utils::*;

use bollard::{
    container::{ListContainersOptions, LogsOptions},
    Docker,
};
use chrono::{offset::Utc, Duration};
use futures::StreamExt;
use k8s_openapi::api::core::v1 as corev1;
use ::kube::{
    api::{Api, LogParams},
    Client,
};

pub async fn process_logs(namespace: Option<&str>, container: Option<&str>, since: Option<u8>, follow: bool) -> Result<()>
{
    let since: i64 = since.unwrap_or(1).into();

    if namespace.is_none() {
        let docker = Docker::connect_with_local_defaults()?;

        let options = Some(ListContainersOptions::default());
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

        let options = Some(LogsOptions::<String> {
            since: (Utc::now() - Duration::hours(since)).timestamp(),
            stdout: true,
            stderr: true,
            follow,
            ..Default::default()
        });
        let logs = docker.logs(&container_name, options);

        self::docker::parse_docker_stream(logs.boxed()).await?;
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
                    since_seconds: Some(since * 60 * 60),
                    follow,
                    ..LogParams::default()
                },
            )
            .await?;

        self::kube::parse_kube_stream(logs.boxed()).await?;
    }

    Ok(())
}
