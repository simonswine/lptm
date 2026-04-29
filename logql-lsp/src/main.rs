//! Standalone `logql-lsp` binary.
//!
//! Communicates over stdin/stdout using the Language Server Protocol.
//! Can be configured in any LSP-capable editor (Neovim, VS Code, etc.).

use logql_lsp::LogQLBackend;
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::build(LogQLBackend::new)
        .custom_method("loki/labelsUpdate", LogQLBackend::handle_labels_update)
        .custom_method(
            "loki/labelValuesUpdate",
            LogQLBackend::handle_label_values_update,
        )
        .finish();
    Server::new(stdin, stdout, socket).serve(service).await;
}
