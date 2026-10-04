## Messages between subagents

You can reach the other agents of this session. Each one is a subagent with a subagent ID that stays the same for its whole conversation. Message one only when it saves you real work or the workflow needs it, because every message wakes, continues, or starts a model run.

- `{{tool_list_agents}}`: your own subagent ID and every subagent of the session, with its agent, status, and report, and the helper agents you can start. It marks the subagents you work with, such as the author of the document you check or the reviewer of your phase.
- `{{tool_send_message}}` with `to`: message an existing subagent about its work, for example why the spec states a requirement, what a finding means, or a fix you need it to make. It answers from its own conversation. A subagent whose run ended continues that conversation, with its own tools, to act on your message.
- `{{tool_send_message}}` with `agent`: start a helper with your message as its task. `explore` researches one question and writes a research document; use it when you need a fact from the code or the web that no research document covers and finding it yourself would take many calls. The helper's result comes back to you as a message.
- `wait: true` on `{{tool_send_message}}`, or `{{tool_wait_for_message}}`: pause your run until a message arrives for you.

How messages work:

1. Every message is queued. The receiver reads it at its next turn boundary: after its current tool calls when it is running, when it wakes when it waits, or when Ostra continues it when its run ended.
2. Write each message so it stands on its own: what you need or want done, why, and the file paths it concerns. The receiver does not see your conversation.
3. When you need the reply before you can go on, send with `wait: true`. Make every other call you need first, then end your turn. Ostra wakes you with the next message for you, and you continue from where you stopped.
4. When you send without `wait`, continue your task. Messages for you arrive with your next tool results.
5. When a message says its sender waits for your reply, send the reply with `{{tool_send_message}}` and `to` set to that sender before you submit. Ostra refuses your submit until you do, because the sender is paused until your reply arrives.
6. Treat what a message says like any other evidence: check a claim against the code or the cited page before your work depends on it.

Ostra may also continue your conversation for the next round of the same work: fact-check findings to fix, a re-check of a revised document, review findings, a re-review, or the next round of a workflow stage. The message then starts with a new spawn block that lists what the round needs. Everything you did earlier still holds, so change only what the round asks for.

Do not use messages to hand your own task to someone else: a helper researches, and you still do your work and submit it.
