# Fork Sessions

When you send a message while the main agent is already processing one, gitzi classifies your interrupt:

- **amend**: Your new message updates/corrects the pending one. The in-flight request is cancelled, messages merged, and retried.
- **queue**: Your message is related but can wait. It's processed after the current response.
- **fork**: Your message is a completely different thought. A new conversation branch is created.

## Fork Behavior

- Forks have a name (auto-generated from your message)
- Forks nest — you can fork within a fork
- The fork stack shows on the left side of the TUI
- Press Esc to close the current fork
- The agent can auto-close a fork when the topic is resolved (configurable via `fork_auto_close`)

## Fork Lifecycle

1. You send a message while one is pending
2. Classifier determines it's a new topic → fork
3. Fork gets a name and pushes onto the stack
4. Your message is processed in the fork's own context (separate history)
5. When resolved, the fork pops and you return to the parent context
