//! Passes local context to an ACP process without a hosted attachment service.
use super::protocol::ContentBlock;
use crate::ai::agent::AIAgentAttachment;
use anyhow::{Result, bail};

pub(super) struct AttachmentResolver;
impl AttachmentResolver {
    pub(super) async fn prompt_content(
        &self,
        text: String,
        attachments: Vec<AIAgentAttachment>,
    ) -> Result<Vec<ContentBlock>> {
        prompt_content(text, attachments)
    }
}
fn prompt_content(text: String, attachments: Vec<AIAgentAttachment>) -> Result<Vec<ContentBlock>> {
    let mut blocks = vec![ContentBlock::Text { text }];
    for attachment in attachments {
        let block = match attachment {
            AIAgentAttachment::PlainText(text)
            | AIAgentAttachment::DocumentContent { content: text, .. } => {
                ContentBlock::Text { text }
            }
            AIAgentAttachment::FilePathReference {
                file_path,
                file_name,
                ..
            } => {
                let path = std::fs::canonicalize(file_path)?;
                ContentBlock::ResourceLink {
                    uri: url::Url::from_file_path(path)
                        .map_err(|_| anyhow::anyhow!("attachment needs an absolute local path"))?
                        .into(),
                    name: Some(file_name),
                }
            }
            AIAgentAttachment::DriveObject { .. } => {
                bail!("hosted Drive attachments are unavailable")
            }
            local => ContentBlock::Text {
                text: serde_json::to_string(&local)?,
            },
        };
        blocks.push(block);
    }
    Ok(blocks)
}
#[cfg(test)]
#[path = "attachments_tests.rs"]
mod tests;
