#![allow(clippy::result_large_err)]

use anyhow::{Context, Result};
use extension_protocol::error::{ErrorCode, ErrorData, ProtocolError, error_codes};
use interprocess::local_socket::{
    GenericNamespaced, ToNsName,
    tokio::{Stream, prelude::*},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::driver::MyCompatDriver;

pub async fn run(socket_name: &str) -> Result<()> {
    let name = socket_name
        .to_ns_name::<GenericNamespaced>()
        .context("invalid local socket name")?;
    let stream = Stream::connect(name)
        .await
        .context("failed to connect to host listener")?;
    let (reader, writer) = tokio::io::split(stream);
    extension_driver::serve(MyCompatDriver::new(), reader, writer).await
}

pub async fn handle_stream<R, W>(reader: R, writer: W) -> Result<()>
where
    R: AsyncReadExt + Unpin + Send,
    W: AsyncWriteExt + Unpin + Send + 'static,
{
    extension_driver::serve(MyCompatDriver::new(), reader, writer).await
}

pub(crate) fn invalid_params(message: impl Into<String>) -> ProtocolError {
    ProtocolError::new(error_codes::INVALID_PARAMS, message)
}

pub(crate) fn params_deserialize_error(error: serde_json::Error) -> ProtocolError {
    ProtocolError::new(
        error_codes::INVALID_PARAMS,
        format!("failed to deserialize params: {error}"),
    )
}

pub(crate) fn protocol_error_from_anyhow(code: ErrorCode, error: anyhow::Error) -> ProtocolError {
    let chain = error
        .chain()
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    let data = ErrorData::new();
    let extra = serde_json::json!({ "chain": chain });

    ProtocolError::new(code, format!("{error:#}")).with_data(data.with_extra(extra))
}

pub(crate) fn not_supported(message: impl Into<String>) -> ProtocolError {
    ProtocolError::new(error_codes::METHOD_NOT_FOUND, message)
}

/// 默认入口：读取宿主传入的 socket 名（环境变量或首参数），缺省使用
/// `navop-pgcompat-driver.sock`。每个 [[bin]] 产品都走这里。
pub async fn run_default() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let socket_name = std::env::var("ONETCLI_EXT_SOCKET")
        .ok()
        .or_else(|| std::env::args().nth(1))
        .unwrap_or_else(|| "navop-pgcompat-driver.sock".to_string());

    run(&socket_name).await
}
