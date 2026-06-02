//! Render the attachment class `AIAgentContext` that comes with a single `AIAgentInput::UserQuery` as
//! The user message content sent to the upstream model (text prefix + binary multimodal component).
//!
//! ## Alignment with warp's own path
//!
//! Under warp's own protocol, these attachments go through `api::InputContext`'s `executed_shell_commands/
//! selected_text/files/images` field (see `app/src/ai/agent/api/convert_to.rs`
//! `convert_context`). BYOP directly connected to OpenAI / Anthropic / Gemini / Ollama compatible
//! `/chat/completions`, without the `InputContext` layer structure, can only embed data into user messages.
//!
//! Fields are strictly aligned with warp protobuf and no extra-protocol fields are introduced:
//!
//! | type | warp protobuf field |
//! |------------------|-------------------------------------------------------------|
//! | `Block`          | command / output / exit_code / command_id / is_auto_attached / started_ts / finished_ts |
//! | `SelectedText`   | text                                                        |
//! | `File`(text)    | file_name / content / line_range                            |
//! | `File`(binary) | file_name / data / mime_type(P1 binary channel) |
//! | `Image` | mime_type / file_name / data(P1 binary channel) |
//!
//! ## Scope: per-input, does not affect system prompt
//!
//! - These attachments only inject user messages, not system prompts
//! - The `referenced_attachments` of the history round will be re-rendered from the persistent message to avoid losing context in multiple rounds of BYOP
//! - env/git/skills/project_rules/codebase/current_time these **environmental** contexts
//!   It is still rendered into the system by `prompt_renderer` and does not overlap with this module.

use std::collections::HashMap;

use base64::Engine;

use crate::ai::agent::{AIAgentAttachment, AIAgentContext, DriveObjectPayload, ImageContext};
use crate::ai::block_context::BlockContext;
use ai::agent::action_result::{AnyFileContent, FileContext};
use warp_multi_agent_api as api;

/// Dual channel results returned by `collect_user_attachments`.
///
/// - `prefix`: text prefix block, prepend to user message text. contains block/selected_text/
///   Inline XML for text-like files, and placeholder hints for binary attachments (allowing LLM to reference the file name).
/// - `binaries`: Attachments that need to be injected into multimodal messages as `ContentPart::Binary`
///   (image / PDF / audio). caller(chat_stream.rs) will be filtered by model capability
///   Determines whether to switch to `MessageContent::Parts`.
#[derive(Debug, Default, Clone)]
pub struct UserAttachments {
    pub prefix: Option<String>,
    pub binaries: Vec<UserBinary>,
}

/// A binary attachment, equivalent to the input triple of genai `Binary::from_base64`.
#[derive(Debug, Clone)]
pub struct UserBinary {
    pub name: String,
    pub content_type: String,
    /// Base64 encoded data (without `data:` prefix).
    pub data: String,
}

impl UserAttachments {
    pub fn is_empty(&self) -> bool {
        self.prefix.is_none() && self.binaries.is_empty()
    }
}

/// The attachment context for rendering a single UserQuery is "text prefix + binary component".
///
/// The caller should:
/// 1. Prefix `prefix` before the user query text (leave a blank line in the middle)
/// 2. Filter `binaries` according to model capability, and switch to `MessageContent::Parts` if there are reservations.
pub fn collect_user_attachments(ctx: &[AIAgentContext]) -> UserAttachments {
    let mut blocks: Vec<&BlockContext> = Vec::new();
    let mut selected_texts: Vec<&str> = Vec::new();
    let mut text_files: Vec<&FileContext> = Vec::new();
    let mut binary_files: Vec<&FileContext> = Vec::new();
    let mut images: Vec<&ImageContext> = Vec::new();

    for c in ctx {
        match c {
            AIAgentContext::Block(b) => blocks.push(b),
            AIAgentContext::SelectedText(t) => selected_texts.push(t),
            AIAgentContext::File(f) => match &f.content {
                AnyFileContent::StringContent(_) => text_files.push(f),
                AnyFileContent::BinaryContent(_) => binary_files.push(f),
            },
            AIAgentContext::Image(img) => images.push(img),
            // Environmental context is processed by prompt_renderer and does not enter user messages.
            AIAgentContext::Directory { .. }
            | AIAgentContext::ExecutionEnvironment(_)
            | AIAgentContext::CurrentTime { .. }
            | AIAgentContext::Codebase { .. }
            | AIAgentContext::ProjectRules { .. }
            | AIAgentContext::Git { .. }
            | AIAgentContext::Skills { .. } => {}
        }
    }

    let mut out = UserAttachments::default();

    // ----- prefix -----
    let has_any_prefix_content = !blocks.is_empty()
        || !selected_texts.is_empty()
        || !text_files.is_empty()
        || !binary_files.is_empty()
        || !images.is_empty();
    if has_any_prefix_content {
        let mut prefix = String::with_capacity(256);
        prefix.push_str("<attached_context>\n");
        for b in &blocks {
            render_block(&mut prefix, b);
        }
        for t in &selected_texts {
            render_selected_text(&mut prefix, t);
        }
        for f in &text_files {
            render_file_text(&mut prefix, f);
        }
        for f in &binary_files {
            render_file_binary_placeholder(&mut prefix, f);
        }
        for img in &images {
            render_image_placeholder(&mut prefix, img);
        }
        prefix.push_str("</attached_context>");
        out.prefix = Some(prefix);
    }

    // ----- binaries (for callers to filter by capability and then inject ContentPart::Binary) -----
    for img in &images {
        out.binaries.push(UserBinary {
            name: img.file_name.clone(),
            content_type: img.mime_type.clone(),
            // ImageContext.data is already a base64 string (`process_non_image_files` sibling path
            // `read_and_process_images_async` is done when PendingAttachment::Image is enqueued encoding)
            data: img.data.to_string(),
        });
    }
    for f in &binary_files {
        if let AnyFileContent::BinaryContent(bytes) = &f.content {
            // Empty BinaryContent means "the upper layer intentionally did not read bytes" (.exe / very large file / non-multimodal
            // mime), only use the placeholder XML path, and do not send ContentPart::Binary.
            if bytes.is_empty() {
                continue;
            }
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            // Guess mime using extension on file_name; `mime_guess` with process_non_image_files
            // The same set of rules is followed, and the calculation is repeated here because FileContext does not save mime.
            let mime = mime_guess::from_path(&f.file_name)
                .first_or_octet_stream()
                .to_string();
            out.binaries.push(UserBinary {
                name: f.file_name.clone(),
                content_type: mime,
                data: b64,
            });
        }
    }

    out
}

pub fn render_referenced_attachments(
    referenced_attachments: &HashMap<String, AIAgentAttachment>,
) -> Option<String> {
    if referenced_attachments.is_empty() {
        return None;
    }

    let mut refs = referenced_attachments.iter().collect::<Vec<_>>();
    refs.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut out = String::with_capacity(256);
    out.push_str("<attached_context>\n");
    for (reference, attachment) in refs {
        render_attachment(&mut out, reference, attachment);
    }
    out.push_str("</attached_context>");
    Some(out)
}

pub fn render_api_referenced_attachments(
    referenced_attachments: &HashMap<String, api::Attachment>,
) -> Option<String> {
    if referenced_attachments.is_empty() {
        return None;
    }

    let mut refs = referenced_attachments.iter().collect::<Vec<_>>();
    refs.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut out = String::with_capacity(256);
    out.push_str("<attached_context>\n");
    for (reference, attachment) in refs {
        render_api_attachment(&mut out, reference, attachment);
    }
    out.push_str("</attached_context>");
    Some(out)
}

/// Compatible with old callers: only take the prefix text. Please use `collect_user_attachments` for new code.
#[cfg(test)]
pub fn render_user_attachments(ctx: &[AIAgentContext]) -> Option<String> {
    collect_user_attachments(ctx).prefix
}

// ---------------------------------------------------------------------------
// child renderer
// ---------------------------------------------------------------------------

fn render_block(out: &mut String, b: &BlockContext) {
    use std::fmt::Write;
    let _ = write!(
        out,
        "  <executed_shell_command command_id=\"{}\" exit_code=\"{}\" auto_attached=\"{}\"",
        xml_attr(&String::from(b.id.clone())),
        b.exit_code.value(),
        b.is_auto_attached
    );
    if let Some(ts) = b.started_ts {
        let _ = write!(out, " started_ts=\"{}\"", ts.to_rfc3339());
    }
    if let Some(ts) = b.finished_ts {
        let _ = write!(out, " finished_ts=\"{}\"", ts.to_rfc3339());
    }
    out.push_str(">\n");
    out.push_str("    <command>");
    out.push_str(&xml_text(&b.command));
    out.push_str("</command>\n");
    out.push_str("    <output>");
    out.push_str(&xml_text(&b.output));
    out.push_str("</output>\n");
    out.push_str("  </executed_shell_command>\n");
}

fn render_attachment(out: &mut String, reference: &str, attachment: &AIAgentAttachment) {
    match attachment {
        AIAgentAttachment::PlainText(text) => {
            render_plain_text_attachment(out, reference, text);
        }
        AIAgentAttachment::DocumentContent {
            document_id,
            content,
            line_range,
            ..
        } => {
            let line_range = line_range
                .as_ref()
                .map(|range| (range.start.as_usize(), range.end.as_usize()));
            render_document_content(out, reference, document_id, content, line_range);
        }
        AIAgentAttachment::DriveObject { uid, payload } => {
            render_drive_object(out, reference, uid, payload.as_ref());
        }
        AIAgentAttachment::DiffHunk {
            file_path,
            line_range,
            diff_content,
            lines_added,
            lines_removed,
            ..
        } => {
            use std::fmt::Write;
            let _ = writeln!(
                out,
                "  <diff_hunk reference=\"{}\" file_path=\"{}\" line_start=\"{}\" line_end=\"{}\" lines_added=\"{}\" lines_removed=\"{}\">",
                xml_attr(reference),
                xml_attr(file_path),
                line_range.start.as_usize(),
                line_range.end.as_usize(),
                lines_added,
                lines_removed,
            );
            out.push_str(&xml_text(diff_content));
            if !diff_content.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("  </diff_hunk>\n");
        }
        AIAgentAttachment::DiffSet { file_diffs, .. } => {
            use std::fmt::Write;
            let _ = writeln!(out, "  <diff_set reference=\"{}\">", xml_attr(reference));
            for (file_path, hunks) in file_diffs {
                let _ = writeln!(out, "    <file path=\"{}\">", xml_attr(file_path));
                for hunk in hunks {
                    let _ = writeln!(
                        out,
                        "      <hunk line_start=\"{}\" line_end=\"{}\" lines_added=\"{}\" lines_removed=\"{}\">",
                        hunk.line_range.start.as_usize(),
                        hunk.line_range.end.as_usize(),
                        hunk.lines_added,
                        hunk.lines_removed,
                    );
                    out.push_str(&xml_text(&hunk.diff_content));
                    if !hunk.diff_content.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("      </hunk>\n");
                }
                out.push_str("    </file>\n");
            }
            out.push_str("  </diff_set>\n");
        }
        AIAgentAttachment::FilePathReference { file_path, .. } => {
            use std::fmt::Write;
            let _ = writeln!(
                out,
                "  <file_path_reference reference=\"{}\" path=\"{}\" />",
                xml_attr(reference),
                xml_attr(file_path),
            );
        }
        AIAgentAttachment::Block(block) => {
            render_block(out, block);
        }
    }
}

fn render_api_attachment(out: &mut String, reference: &str, attachment: &api::Attachment) {
    match attachment.value.as_ref() {
        Some(api::attachment::Value::PlainText(text)) => {
            render_plain_text_attachment(out, reference, text);
        }
        Some(api::attachment::Value::DocumentContent(document)) => {
            let line_range = document
                .line_range
                .as_ref()
                .map(|range| (range.start as usize, range.end as usize));
            render_document_content(
                out,
                reference,
                &document.document_id,
                &document.content,
                line_range,
            );
        }
        Some(api::attachment::Value::DriveObject(object)) => {
            render_api_drive_object(out, reference, object);
        }
        Some(api::attachment::Value::ExecutedShellCommand(block)) => {
            let block: BlockContext = block.clone().into();
            render_block(out, &block);
        }
        Some(api::attachment::Value::FilePathReference(file)) => {
            use std::fmt::Write;
            let _ = writeln!(
                out,
                "  <file_path_reference reference=\"{}\" path=\"{}\" />",
                xml_attr(reference),
                xml_attr(&file.file_path),
            );
        }
        _ => {}
    }
}

fn render_plain_text_attachment(out: &mut String, reference: &str, text: &str) {
    use std::fmt::Write;
    let _ = writeln!(out, "  <plain_text reference=\"{}\">", xml_attr(reference),);
    out.push_str(&xml_text(text));
    if !text.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("  </plain_text>\n");
}

fn render_document_content(
    out: &mut String,
    reference: &str,
    document_id: &str,
    content: &str,
    line_range: Option<(usize, usize)>,
) {
    use std::fmt::Write;
    let _ = write!(
        out,
        "  <document_content reference=\"{}\" document_id=\"{}\"",
        xml_attr(reference),
        xml_attr(document_id),
    );
    if let Some((line_start, line_end)) = line_range {
        let _ = write!(
            out,
            " line_start=\"{}\" line_end=\"{}\"",
            line_start, line_end,
        );
    }
    out.push_str(">\n");
    out.push_str(&xml_text(content));
    if !content.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("  </document_content>\n");
}

fn render_drive_object(
    out: &mut String,
    reference: &str,
    uid: &str,
    payload: Option<&DriveObjectPayload>,
) {
    use std::fmt::Write;
    match payload {
        Some(DriveObjectPayload::Workflow {
            name,
            description,
            command,
        }) => {
            let _ = writeln!(
                out,
                "  <drive_object reference=\"{}\" uid=\"{}\" type=\"workflow\">",
                xml_attr(reference),
                xml_attr(uid),
            );
            render_named_text(out, "name", name);
            render_named_text(out, "description", description);
            render_named_text(out, "command", command);
            out.push_str("  </drive_object>\n");
        }
        Some(DriveObjectPayload::Notebook { title, content }) => {
            let _ = writeln!(
                out,
                "  <drive_object reference=\"{}\" uid=\"{}\" type=\"notebook\">",
                xml_attr(reference),
                xml_attr(uid),
            );
            render_named_text(out, "title", title);
            render_named_text(out, "content", content);
            out.push_str("  </drive_object>\n");
        }
        Some(DriveObjectPayload::GenericStringObject {
            payload,
            object_type,
        }) => {
            let _ = writeln!(
                out,
                "  <drive_object reference=\"{}\" uid=\"{}\" type=\"{}\">",
                xml_attr(reference),
                xml_attr(uid),
                xml_attr(object_type),
            );
            render_named_text(out, "payload", payload);
            out.push_str("  </drive_object>\n");
        }
        None => {
            let _ = writeln!(
                out,
                "  <drive_object reference=\"{}\" uid=\"{}\" />",
                xml_attr(reference),
                xml_attr(uid),
            );
        }
    }
}

fn render_api_drive_object(out: &mut String, reference: &str, object: &api::DriveObject) {
    use std::fmt::Write;
    match object.object_payload.as_ref() {
        Some(api::drive_object::ObjectPayload::Workflow(workflow)) => {
            let _ = writeln!(
                out,
                "  <drive_object reference=\"{}\" uid=\"{}\" type=\"workflow\">",
                xml_attr(reference),
                xml_attr(&object.uid),
            );
            render_named_text(out, "name", &workflow.name);
            render_named_text(out, "description", &workflow.description);
            render_named_text(out, "command", &workflow.command);
            out.push_str("  </drive_object>\n");
        }
        Some(api::drive_object::ObjectPayload::Notebook(notebook)) => {
            let _ = writeln!(
                out,
                "  <drive_object reference=\"{}\" uid=\"{}\" type=\"notebook\">",
                xml_attr(reference),
                xml_attr(&object.uid),
            );
            render_named_text(out, "title", &notebook.title);
            render_named_text(out, "content", &notebook.content);
            out.push_str("  </drive_object>\n");
        }
        Some(api::drive_object::ObjectPayload::GenericStringObject(generic)) => {
            let _ = writeln!(
                out,
                "  <drive_object reference=\"{}\" uid=\"{}\" type=\"{}\">",
                xml_attr(reference),
                xml_attr(&object.uid),
                xml_attr(&generic.object_type),
            );
            render_named_text(out, "payload", &generic.payload);
            out.push_str("  </drive_object>\n");
        }
        None => {
            let _ = writeln!(
                out,
                "  <drive_object reference=\"{}\" uid=\"{}\" />",
                xml_attr(reference),
                xml_attr(&object.uid),
            );
        }
    }
}

fn render_named_text(out: &mut String, tag_name: &str, text: &str) {
    use std::fmt::Write;
    let _ = writeln!(out, "    <{tag_name}>");
    out.push_str(&xml_text(text));
    if !text.ends_with('\n') {
        out.push('\n');
    }
    let _ = writeln!(out, "    </{tag_name}>");
}

fn render_selected_text(out: &mut String, t: &str) {
    out.push_str("  <selected_text>");
    out.push_str(&xml_text(t));
    out.push_str("</selected_text>\n");
}

fn render_file_text(out: &mut String, f: &FileContext) {
    use std::fmt::Write;
    let path = xml_attr(&f.file_name);
    let content = match &f.content {
        AnyFileContent::StringContent(content) => content.as_str(),
        AnyFileContent::BinaryContent(_) => return, // shouldn't happen, dispatched away above
    };
    let _ = write!(out, "  <file path=\"{path}\"");
    if let Some(range) = &f.line_range {
        let _ = write!(
            out,
            " line_start=\"{}\" line_end=\"{}\"",
            range.start, range.end
        );
    }
    out.push_str(">\n");
    out.push_str(&xml_text(content));
    if !content.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("  </file>\n");
}

/// Binary file prefix placeholder: Let LLM know that this file, full path and mime exist, and can decide:
/// - Read ContentPart::Binary directly (model supports + bytes uploaded)
/// - Or use the read_files / shell tool to process it yourself according to the path (.exe / .zip / very large files)
///
/// The actual bytes are passed through the caller side `MessageContent::Parts` to ContentPart::Binary,
/// **Don't** repeat base64 here (to avoid double tokens + many models are slow to parse ultra-long base64).
fn render_file_binary_placeholder(out: &mut String, f: &FileContext) {
    use std::fmt::Write;
    let path = xml_attr(&f.file_name);
    // mime is pushed from the file name/extension via mime_guess, consistent with process_non_image_files.
    let mime = mime_guess::from_path(&f.file_name)
        .first_or_octet_stream()
        .to_string();
    let size = match &f.content {
        AnyFileContent::BinaryContent(bytes) if !bytes.is_empty() => Some(bytes.len()),
        // bytes is empty (very large file / non-multimodal binary) → size is unknown, the size attribute is not output,
        // Let the AI ​​check it by itself using read_files / Get-Item or the like.
        _ => None,
    };
    if let Some(size) = size {
        let _ = writeln!(
            out,
            "  <file path=\"{path}\" mime_type=\"{}\" binary=\"true\" size=\"{size}\" />",
            xml_attr(&mime)
        );
    } else {
        let _ = writeln!(
            out,
            "  <file path=\"{path}\" mime_type=\"{}\" binary=\"true\" />",
            xml_attr(&mime)
        );
    }
}

/// Image prefix placeholder: has the same semantics as binary file, the actual data enters multi-modality through ContentPart::Binary.
fn render_image_placeholder(out: &mut String, img: &ImageContext) {
    use std::fmt::Write;
    let _ = writeln!(
        out,
        "  <image file_name=\"{}\" mime_type=\"{}\" />",
        xml_attr(&img.file_name),
        xml_attr(&img.mime_type),
    );
}

// ---------------------------------------------------------------------------
// XML escaping
// ---------------------------------------------------------------------------

fn xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_attr(s: &str) -> String {
    xml_text(s).replace('"', "&quot;")
}

#[cfg(test)]
#[path = "user_context_tests.rs"]
mod tests;
