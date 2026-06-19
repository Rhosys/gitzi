use crate::model::Task;

pub const DEFAULT_PREAMBLE: &str = "You are a disciplined coding agent with one rule above all others: \
    do the smallest change that satisfies the task. Nothing more.\n\n\
    Rules:\n\
    - Implement only what the task explicitly states. No refactoring, no cleanup, \
      no \"while I'm here\" changes.\n\
    - If anything about the task is unclear, ask ONE simple question and stop. \
      Do not guess. Do not fill in gaps.\n\
    - Never attempt to finish quickly. A slow correct step beats a fast wrong one.\n\
    - When done, commit only the files you changed for this task.\n\
    - If any gitzi_* tool call returns a 401 Unauthorized error, your session \
      token has expired. Immediately stop all work and run: Bash(\"exit 1\")";

/// Build the task-content portion of a pipeline agent prompt: title,
/// description, answered review-queue questions, resume-from-previous-session
/// state, and rejection feedback. Does not include the preamble/system prompt —
/// callers that have a separate system-message channel (e.g. `rig`'s
/// `.preamble()`) should send that independently; callers without one
/// (e.g. the `claude` CLI subprocess) should prepend it themselves.
pub fn build_task_content(
    task: &Task,
    resume_summary: Option<&str>,
    answered_questions: &[(String, String)],
) -> String {
    let mut prompt = format!("Task: {}\n", task.title);

    if let Some(desc) = &task.description {
        prompt.push_str(&format!("\nDescription:\n{desc}\n"));
    }

    // Inject answers from the human review queue. These are decisions already
    // made — implement them directly, do not raise the same question again.
    if !answered_questions.is_empty() {
        prompt.push_str("\nDecisions already made for this task (implement these, do not re-ask):\n");
        for (question, answer) in answered_questions {
            prompt.push_str(&format!("Q: {question}\nA: {answer}\n\n"));
        }
    }

    if let Some(summary) = resume_summary {
        prompt.push_str(&format!(
            "\nResuming from previous session. Existing work state:\n{summary}\n\
            Review this state before making changes. Continue from where the previous session left off.\n"
        ));
    }

    if let Some(feedback) = &task.agent_feedback {
        prompt.push_str(&format!(
            "\nPrevious attempt was rejected. Feedback from reviewer:\n{feedback}\n\
            Address only the feedback. Do not change anything else.\n"
        ));
    }

    prompt
}
