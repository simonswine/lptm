//! LogQL Language Server backend implementing the tower-lsp `LanguageServer` trait.

use std::collections::HashMap;

use dashmap::DashMap;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    CompletionTextEdit, Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidOpenTextDocumentParams, InitializeParams, InitializeResult, InitializedParams, MessageType,
    Position, Range, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextEdit, Url, WorkDoneProgressOptions,
};
use tower_lsp::{Client, LanguageServer};

use logql_core::analyzer::{analyze, Severity};
use logql_core::completions::{
    compute_completion_items, detect_cursor_context, CompletionKind, CursorContext,
};
use logql_core::parser::parse;

/// Custom notification sent from the TUI client to push Loki label names.
///
/// Method: `loki/labelsUpdate`
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct LabelsUpdateParams {
    /// Stream selector the label names are scoped to (empty = global).
    pub selector: String,
    /// Label names for the given selector.
    pub names: Vec<String>,
}

/// Custom notification for label values.
///
/// Method: `loki/labelValuesUpdate`
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct LabelValuesUpdateParams {
    pub label: String,
    pub selector: String,
    pub values: Vec<String>,
}

// ── Backend ──────────────────────────────────────────────────────────────────

pub struct LogQLBackend {
    pub client: Client,
    /// Stored document text, keyed by URI string.
    documents: DashMap<String, (String, i32)>,
    /// Label names cache: selector → names. Empty selector = global.
    label_names: DashMap<String, Vec<String>>,
    /// Label values cache: (label, selector) → values.
    label_values: DashMap<(String, String), Vec<String>>,
}

impl LogQLBackend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: DashMap::new(),
            label_names: DashMap::new(),
            label_values: DashMap::new(),
        }
    }

    /// Handle the custom `loki/labelsUpdate` notification.
    pub async fn handle_labels_update(&self, params: LabelsUpdateParams) {
        self.label_names.insert(params.selector, params.names);
    }

    /// Handle the custom `loki/labelValuesUpdate` notification.
    pub async fn handle_label_values_update(&self, params: LabelValuesUpdateParams) {
        self.label_values
            .insert((params.label, params.selector), params.values);
    }

    /// Run diagnostics for a document and publish them.
    async fn run_diagnostics(&self, uri: Url, text: &str, version: Option<i32>) {
        let result = parse(text);
        let mut diagnostics: Vec<Diagnostic> = Vec::new();

        for error in &result.errors {
            let range = byte_range_to_lsp(text, error.span.start, error.span.end);
            diagnostics.push(Diagnostic {
                range,
                severity: Some(DiagnosticSeverity::ERROR),
                message: error.message.clone(),
                source: Some("logql-lsp".to_string()),
                ..Default::default()
            });
        }

        if let Some(ref expr) = result.expr {
            for diag in analyze(expr) {
                let range = byte_range_to_lsp(text, diag.span.start, diag.span.end);
                diagnostics.push(Diagnostic {
                    range,
                    severity: Some(match diag.severity {
                        Severity::Error => DiagnosticSeverity::ERROR,
                        Severity::Warning => DiagnosticSeverity::WARNING,
                    }),
                    message: diag.message,
                    source: Some("logql-lsp".to_string()),
                    ..Default::default()
                });
            }
        }

        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }
}

// ── LanguageServer impl ──────────────────────────────────────────────────────

#[tower_lsp::async_trait]
impl LanguageServer for LogQLBackend {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        "{".to_string(),
                        ",".to_string(),
                        "=".to_string(),
                        "|".to_string(),
                        "[".to_string(),
                        "\"".to_string(),
                    ]),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "logql-lsp initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text.clone();
        let version = params.text_document.version;
        self.documents
            .insert(uri.to_string(), (text.clone(), version));
        self.run_diagnostics(uri, &text, Some(version)).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        // FULL sync — use the last change which contains the full text.
        if let Some(change) = params.content_changes.into_iter().last() {
            let text = change.text;
            let version = params.text_document.version;
            self.documents
                .insert(uri.to_string(), (text.clone(), version));
            self.run_diagnostics(uri, &text, Some(version)).await;
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri.to_string();
        let pos = params.text_document_position.position;

        let text = match self.documents.get(&uri) {
            Some(entry) => entry.value().0.clone(),
            None => return Ok(None),
        };

        // Convert LSP position (line/character in UTF-16) to byte offset.
        let cursor_byte = lsp_position_to_byte(&text, pos);

        // Build snapshot caches to pass to the completion engine.
        let names_cache: HashMap<String, Vec<String>> = self
            .label_names
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        let values_cache: HashMap<(String, String), Vec<String>> = self
            .label_values
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();

        let char_pos = cursor_byte_to_char(&text, cursor_byte);
        let ctx = detect_cursor_context(&text, char_pos);
        let items = compute_completion_items(&ctx, &names_cache, &values_cache, 20);

        if items.is_empty() {
            return Ok(None);
        }

        // Replacement range: from start of the current word/prefix to the cursor.
        let prefix_byte = prefix_start_byte(&text, cursor_byte);
        let replace_start = byte_to_lsp_position(&text, prefix_byte);
        let cursor_pos = byte_to_lsp_position(&text, cursor_byte);

        // For LabelValue the prefix starts after the opening quote character.
        let (replace_start, replace_end) = match &ctx {
            CursorContext::LabelValue { prefix, .. } => {
                let quote_byte = cursor_byte.saturating_sub(prefix.len());
                (byte_to_lsp_position(&text, quote_byte), cursor_pos)
            }
            _ => (replace_start, cursor_pos),
        };

        let lsp_items: Vec<CompletionItem> = items
            .into_iter()
            .map(|item| {
                let kind = completion_kind_to_lsp(item.kind);
                let text_edit = CompletionTextEdit::Edit(TextEdit {
                    range: Range {
                        start: replace_start,
                        end: replace_end,
                    },
                    new_text: item.label.clone(),
                });
                CompletionItem {
                    label: item.label.clone(),
                    kind: Some(kind),
                    detail: item.detail,
                    documentation: item
                        .documentation
                        .map(tower_lsp::lsp_types::Documentation::String),
                    sort_text: Some(format!("{:04}", item.sort_priority)),
                    text_edit: Some(text_edit),
                    ..Default::default()
                }
            })
            .collect();

        Ok(Some(CompletionResponse::Array(lsp_items)))
    }
}

// ── Position helpers ─────────────────────────────────────────────────────────

/// Convert a byte offset in `text` to an LSP `Position` (line/UTF-16-char).
pub fn byte_to_lsp_position(text: &str, byte_offset: usize) -> Position {
    let byte_offset = byte_offset.min(text.len());
    let before = &text[..byte_offset];
    let line = before.chars().filter(|&c| c == '\n').count() as u32;
    let last_nl = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col_utf16 = before[last_nl..]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum::<u32>();
    Position {
        line,
        character: col_utf16,
    }
}

/// Convert an LSP `Position` to a byte offset in `text`.
pub fn lsp_position_to_byte(text: &str, pos: Position) -> usize {
    let mut line = 0u32;
    let mut line_start = 0usize;
    for (i, c) in text.char_indices() {
        if line == pos.line {
            break;
        }
        if c == '\n' {
            line += 1;
            line_start = i + c.len_utf8();
        }
    }
    // Walk UTF-16 code units within the line.
    let line_text = &text[line_start..];
    let mut utf16_col = 0u32;
    for (i, c) in line_text.char_indices() {
        if utf16_col >= pos.character {
            return line_start + i;
        }
        utf16_col += c.len_utf16() as u32;
    }
    line_start + line_text.len()
}

/// Convert a byte offset to a char-indexed position (for logql-core APIs).
fn cursor_byte_to_char(text: &str, byte_offset: usize) -> usize {
    text[..byte_offset.min(text.len())].chars().count()
}

/// Convert a byte range to an LSP `Range`.
fn byte_range_to_lsp(text: &str, start: usize, end: usize) -> Range {
    Range {
        start: byte_to_lsp_position(text, start),
        end: byte_to_lsp_position(text, end),
    }
}

/// Find the byte offset where the current word/prefix starts.
fn prefix_start_byte(text: &str, cursor_byte: usize) -> usize {
    let before = &text[..cursor_byte.min(text.len())];
    let bytes = before.as_bytes();
    let mut i = bytes.len();
    while i > 0
        && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b':')
    {
        i -= 1;
    }
    i
}

/// Map logql-core `CompletionKind` to LSP `CompletionItemKind`.
fn completion_kind_to_lsp(kind: CompletionKind) -> CompletionItemKind {
    match kind {
        CompletionKind::LabelName => CompletionItemKind::FIELD,
        CompletionKind::LabelValue => CompletionItemKind::VALUE,
        CompletionKind::Function => CompletionItemKind::FUNCTION,
        CompletionKind::AggregationOp => CompletionItemKind::FUNCTION,
        CompletionKind::PipelineKeyword => CompletionItemKind::KEYWORD,
        CompletionKind::Duration => CompletionItemKind::UNIT,
        CompletionKind::Keyword => CompletionItemKind::KEYWORD,
    }
}
