use super::*;

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
