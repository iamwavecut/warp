use super::*;

#[test]
fn local_text_is_embedded_only_when_the_adapter_accepts_context() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "local context").unwrap();
    let attachment = || AIAgentAttachment::FilePathReference {
        file_id: "local".into(),
        file_name: "notes.txt".into(),
        file_path: path.display().to_string(),
    };
    let resolver = AttachmentResolver::default();
    let baseline =
        futures::executor::block_on(resolver.prompt_content("q".into(), vec![attachment()]))
            .unwrap();
    assert!(matches!(&baseline[1], ContentBlock::ResourceLink { .. }));
    resolver.set_prompt_capabilities(PromptCapabilities {
        embedded_context: true,
        ..Default::default()
    });
    let capable =
        futures::executor::block_on(resolver.prompt_content("q".into(), vec![attachment()]))
            .unwrap();
    assert!(
        matches!(&capable[1], ContentBlock::Resource { resource } if resource.text.as_deref() == Some("local context") && resource.uri.starts_with("file:"))
    );
}

#[test]
fn local_images_follow_capabilities_and_large_text_stays_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("image.png");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\nimage").unwrap();
    let text = dir.path().join("large.txt");
    std::fs::write(&text, vec![b'x'; MAX_EMBEDDED_TEXT_BYTES + 1]).unwrap();
    let attachments = vec![(image, "image.png"), (text, "large.txt")]
        .into_iter()
        .map(|(path, name)| AIAgentAttachment::FilePathReference {
            file_id: "local".into(),
            file_name: name.into(),
            file_path: path.display().to_string(),
        })
        .collect();
    let resolver = AttachmentResolver::default();
    resolver.set_prompt_capabilities(PromptCapabilities {
        image: true,
        embedded_context: true,
        ..Default::default()
    });
    let blocks =
        futures::executor::block_on(resolver.prompt_content("q".into(), attachments)).unwrap();
    assert!(
        matches!(&blocks[1], ContentBlock::Image { mime_type, data } if mime_type == "image/png" && !data.is_empty())
    );
    assert!(matches!(&blocks[2], ContentBlock::ResourceLink { .. }));
}

#[test]
fn local_attachment_paths_and_text_need_no_download() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a b.md");
    std::fs::write(&file, "local").unwrap();
    let blocks = prompt_content(
        "question".into(),
        vec![
            AIAgentAttachment::PlainText("context".into()),
            AIAgentAttachment::FilePathReference {
                file_id: "local".into(),
                file_name: "a b.md".into(),
                file_path: file.display().to_string(),
            },
        ],
    )
    .unwrap();
    assert_eq!(blocks.len(), 3);
    assert!(matches!(&blocks[1], ContentBlock::Text { text } if text.contains("context")));
    assert!(
        matches!(&blocks[2], ContentBlock::ResourceLink { uri, .. } if uri.starts_with("file:") && uri.contains("a%20b.md"))
    );
    assert!(
        prompt_content(
            "question".into(),
            vec![AIAgentAttachment::DriveObject {
                uid: "remote".into(),
                payload: None
            }]
        )
        .is_err()
    );
}
