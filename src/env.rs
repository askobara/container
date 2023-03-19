use anyhow::Result;
use futures::StreamExt;
use crate::utils::*;
use k8s_openapi::api::core::v1 as corev1;
use ::kube::{
    api::{Api, AttachParams, AttachedProcess},
    Client,
};

pub async fn process_env(namespace: Option<&str>, container: Option<&str>) -> Result<()> {
    let client = Client::try_default().await?;
    let all_ns: Api<corev1::Namespace> = Api::all(client.clone());
    let ns = kube_select_one(&all_ns, namespace.as_deref()).await?;

    let pods: Api<corev1::Pod> = Api::namespaced(client, &ns);
    let pod = kube_select_one(&pods, container.as_deref()).await?;

    let attached = pods.exec(&pod, vec!["env"], &AttachParams::default().stderr(false)).await?;

    let output = get_output(attached).await;
    println!("{output}");

    Ok(())
}

async fn get_output(mut attached: AttachedProcess) -> String {
    let stdout = tokio_util::io::ReaderStream::new(attached.stdout().unwrap());
    let out = stdout
        .filter_map(|r| async { r.ok().and_then(|v| String::from_utf8(v.to_vec()).ok()) })
        .collect::<Vec<_>>()
        .await
        .join("");
    attached.join().await.unwrap();
    out
}
