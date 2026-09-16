//! Native HTTP transport. Unix identity is checked on the connected stream.

use std::path::Path;
use std::time::Duration;

use super::DaemonClientError;

const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'/')
    .add(b'?')
    .add(b'#')
    .add(b'%')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

pub(crate) fn path_segment(
    value: &str,
) -> Result<percent_encoding::PercentEncode<'_>, DaemonClientError> {
    // URL parsers normalize even percent-encoded dot segments.
    if matches!(value, "" | "." | "..") {
        return Err(DaemonClientError::InvalidPathSegment);
    }
    Ok(percent_encoding::utf8_percent_encode(value, PATH_SEGMENT))
}

pub(crate) fn local_socket_path() -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::session::get_app_dir()?
        .canonicalize()?
        .join("daemon")
        .join("api.sock"))
}

pub(crate) fn prepare_runtime_directory(parent: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    match std::fs::DirBuilder::new().mode(0o700).create(parent) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let owner = nix::unistd::geteuid().as_raw();
    for ancestor in parent.ancestors() {
        let meta = std::fs::symlink_metadata(ancestor)?;
        let trusted_sticky = meta.uid() == 0 && meta.mode() & 0o1000 != 0;
        anyhow::ensure!(
            meta.is_dir()
                && !meta.file_type().is_symlink()
                && (meta.uid() == owner || meta.uid() == 0)
                && (meta.mode() & 0o022 == 0 || trusted_sticky),
            "Unsafe daemon socket directory: {}",
            ancestor.display()
        );
    }
    let directory = std::fs::symlink_metadata(parent)?;
    anyhow::ensure!(
        directory.uid() == owner && directory.mode() & 0o777 == 0o700,
        "Daemon socket directory must be owned by this user with mode0700"
    );
    Ok(())
}

pub(crate) async fn connect_unix(path: &Path) -> Result<tokio::net::UnixStream, DaemonClientError> {
    let stream = tokio::net::UnixStream::connect(path)
        .await
        .map_err(|_| DaemonClientError::UnixTransport)?;
    let uid =
        crate::process::unix_peer_uid(&stream).map_err(|_| DaemonClientError::PeerIdentity)?;
    if uid != nix::unistd::geteuid().as_raw() {
        return Err(DaemonClientError::PeerIdentity);
    }
    Ok(stream)
}

pub(crate) async fn execute(
    http: &reqwest::Client,
    unix_path: Option<&Path>,
    request: reqwest::Request,
) -> Result<reqwest::Response, DaemonClientError> {
    let Some(path) = unix_path else {
        return http
            .execute(request)
            .await
            .map_err(|_| DaemonClientError::Transport);
    };
    tokio::time::timeout(Duration::from_secs(15), async {
        let stream = connect_unix(path).await?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
                .await
                .map_err(|_| DaemonClientError::UnixTransport)?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let mut request: axum::http::Request<reqwest::Body> = request
            .try_into()
            .map_err(|_: reqwest::Error| DaemonClientError::Transport)?;
        if !request.headers().contains_key(axum::http::header::HOST) {
            request.headers_mut().insert(
                axum::http::header::HOST,
                axum::http::HeaderValue::from_static("localhost"),
            );
        }
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| DaemonClientError::UnixTransport)?;
        let response = response
            .map(|body| reqwest::Body::wrap_stream(axum::body::Body::new(body).into_data_stream()));
        Ok(reqwest::Response::from(response))
    })
    .await
    .map_err(|_| DaemonClientError::Timeout)?
}
