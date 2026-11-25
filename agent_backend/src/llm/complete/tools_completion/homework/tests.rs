use super::*;
use serde_json::json;

use crate::llm::complete::tools_completion::call;

#[test]
fn homework_questions_follow_numbering_and_skip_inner_lists() {
    let pages = vec![
        "Homework 3\nDue Friday. Show all work.\n1. Compute the integral (10 points)\nUse 3.5 as the constant.\n(a) first part\n(b) second part\n".to_string(),
        "2. Prove the lemma\nSteps:\n1. assume\n2. derive\n3. Solve the ODE [5 pts]\n".to_string(),
    ];
    let parsed = split_questions(&pages);
    assert_eq!(parsed.style, QuestionStyle::Arabic);
    let labels: Vec<_> = parsed.questions.iter().map(|q| q.label.as_str()).collect();
    assert_eq!(labels, ["1", "2", "3"]);
    assert!(parsed.preamble.contains("Show all work"));
    assert_eq!(parsed.questions[0].points.as_deref(), Some("10"));
    assert_eq!(parsed.questions[0].subparts, ["(a)", "(b)"]);
    assert_eq!((parsed.questions[1].first_page, parsed.questions[1].last_page), (2, 2));
    assert!(parsed.questions[1].text.contains("2. derive"));
    assert_eq!(parsed.questions[2].points.as_deref(), Some("5"));

    let parsed = split_questions(&["Problem 1\nfoo\n1. inner\nProblem 2 (continued)\nbar\nProblem 2\nbaz".to_string()]);
    assert_eq!(parsed.style, QuestionStyle::Keyword);
    assert_eq!(parsed.questions.len(), 2);
    assert!(parsed.questions[0].text.contains("1. inner"));
    assert!(parsed.questions[1].text.contains("bar") && parsed.questions[1].text.contains("baz"));

    let parsed = split_questions(&["一、选择题\n1. 甲\n2. 乙\n二、计算题（20分）\n1. 丙\n2. 丁".to_string()]);
    assert_eq!(parsed.questions.len(), 4);
    assert_eq!(parsed.questions[2].section.as_deref(), Some("二、计算题（20分）"));
    assert!(!parsed.questions[1].text.contains("计算题"));

    let parsed = split_questions(&["2.1\n2.1.1 a\n1. step\n2.1.2 b\n2.2\n2.2.1 c\nUse 3.5 here\n2.2.2 d".to_string()]);
    assert_eq!(parsed.style, QuestionStyle::Dotted);
    let labels: Vec<_> = parsed.questions.iter().map(|q| q.label.as_str()).collect();
    assert_eq!(labels, ["2.1.1", "2.1.2", "2.2.1", "2.2.2"]);

    let parsed = split_questions(&["写一篇 500 字的读书报告。".to_string()]);
    assert_eq!(parsed.style, QuestionStyle::Whole);
    assert_eq!(parsed.questions.len(), 1);
}

#[test]
fn homework_html_keeps_list_numbers_and_text_becomes_safe_html() {
    let text = html_to_text("<p>Answer:</p><ol><li>A &amp; B</li><li>x &lt; 3&nbsp;ok</li></ol>");
    assert_eq!(text, "Answer:\n\n1. A & B\n2. x < 3 ok");
    let parsed = split_questions(&[text]);
    assert_eq!(parsed.questions.len(), 2);

    assert_eq!(text_to_html("a < b\n  c\n\nd"), "<p>a &lt; b<br>\n&nbsp;&nbsp;c</p>\n<p>d</p>");
    assert_eq!(text_to_html("<p>raw</p>"), "<p>raw</p>");
}

#[tokio::test]
async fn homework_rejects_bad_arguments_before_touching_canvas() {
    let error = call("homework", json!({"action": "questions"})).await.unwrap_err();
    assert!(error.contains("path"));
    let error = call("homework", json!({"action": "submit", "course_id": "1", "assignment_id": "2"})).await.unwrap_err();
    assert!(error.contains("text"));
    let error = call("homework", json!({"action": "submit", "course_id": "1", "assignment_id": "2", "text": "x", "files": ["/tmp/a"]})).await.unwrap_err();
    assert!(error.contains("二选一"));
    let error = call("homework", json!({"action": "submit", "course_id": "1", "assignment_id": "2", "files": ["/nonexistent/answer.pdf"]})).await.unwrap_err();
    assert!(error.contains("不存在"));
    let output = call("homework", json!({"action": "questions", "text": "1. a\n2. b", "question": "2"})).await.unwrap();
    assert!(output.contains("2. b") && !output.contains("1. a"));
}

#[tokio::test]
async fn homework_submits_directly_or_from_the_latest_preview() {
    let file = std::env::temp_dir().join(format!("hw-gate-{}.txt", std::process::id()));
    std::fs::write(&file, "answer v1").unwrap();
    let pending = |course: &str| PendingSubmission {
        course_id: course.to_string(),
        assignment_id: "9".to_string(),
        text: None,
        files: vec![file.clone()],
        files_digest: files_digest(&[file.clone()]).unwrap(),
        comment: None,
        created: std::time::Instant::now(),
    };
    let submit = |course: &str| json!({"action": "submit", "course_id": course, "assignment_id": "9", "confirm": true});
    let has_pending = |course: &str| {
        pending_submissions().lock().unwrap().iter().any(|item| item.is_for(course, "9"))
    };
    // 这些测试不联网：通过内容检查后会停在 Canvas 这一步（缺令牌或请求失败）。
    let reached_canvas = |error: &str| !error.contains("没有可提交的内容") && !error.contains("预览后被修改");

    // 没有预览也没带内容：无从提交
    let error = call("homework", submit("gate-0")).await.unwrap_err();
    assert!(error.contains("没有可提交的内容"), "{error}");

    // 用户明确同意后，不带内容的 confirm=true 提交预览过的那份，且预览只用一次
    pending_submissions().lock().unwrap().push(pending("gate-1"));
    let error = call("homework", submit("gate-1")).await.unwrap_err();
    assert!(reached_canvas(&error), "{error}");
    assert!(!has_pending("gate-1"));
    assert!(call("homework", submit("gate-1")).await.unwrap_err().contains("没有可提交的内容"));

    // 用户一开始就说了直接交：confirm=true 带内容，无需预览
    let mut direct = submit("gate-3");
    direct["text"] = json!("直接提交的答案");
    let error = call("homework", direct).await.unwrap_err();
    assert!(reached_canvas(&error), "{error}");

    // 预览后文件被改动则拒绝
    pending_submissions().lock().unwrap().push(pending("gate-2"));
    std::fs::write(&file, "answer v2").unwrap();
    assert!(call("homework", submit("gate-2")).await.unwrap_err().contains("预览后被修改"));

    let _ = std::fs::remove_file(&file);
}

#[tokio::test]
async fn homework_questions_read_word_documents() {
    let document = r#"<w:document><w:body>
        <w:p><w:r><w:t>第三次作业（请写出过程）</w:t></w:r></w:p>
        <w:p><w:r><w:t>1. 求函数 f(x) = x² 的导数（10分）</w:t></w:r></w:p>
        <w:p><w:r><w:t>2. 证明 √2 是无理数</w:t></w:r></w:p>
        </w:body></w:document>"#;
    let dir = std::env::temp_dir().join(format!("jisjtu-homework-docx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("作业3.docx");
    std::fs::write(&path, crate::llm::complete::tools_completion::office::zip_for_tests(&[("word/document.xml", document, true)])).unwrap();
    let result = call("homework", json!({"action": "questions", "path": path.to_string_lossy()})).await.unwrap();
    assert!(result.contains("识别到 2 道题"), "{result}");
    assert!(result.contains("第 1 题 · 1 · 第 1 页 · 10 分"), "{result}");
    assert!(result.contains("证明 √2 是无理数"), "{result}");
    assert!(result.contains("第三次作业（请写出过程）"), "题前说明保留：{result}");
    std::fs::remove_dir_all(dir).unwrap();
}
