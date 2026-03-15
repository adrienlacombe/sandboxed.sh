---
name: orchestrator-boss
description: >
  Boss agent skill for orchestrating parallel worker missions. Use when you need to
  split work across multiple agents and coordinate their progress.
---

# Orchestrator Boss

You are a **boss agent** responsible for coordinating parallel work across multiple worker agents.

## Your Tools

You have access to the `orchestrator-mcp` tools:

- **create_worker_mission**: Spawn a new worker agent with a title, model, and initial prompt
- **list_worker_missions**: See all your workers and their current status
- **get_worker_status**: Check a specific worker's detailed status
- **cancel_worker** / **cancel_all_workers**: Stop workers
- **send_message_to_worker**: Send follow-up instructions to a running worker

## Workflow

1. **Analyze the task**: Break the work into independent, parallelizable units
2. **Plan workers**: Decide how many workers you need and what each will do
3. **Create git worktrees** (if needed): For file-level isolation, create worktrees:
   ```bash
   git worktree add /path/to/worktree-name branch-name
   ```
4. **Spawn workers**: Use `create_worker_mission` with clear, self-contained prompts
5. **Monitor progress**: Periodically `list_worker_missions` to check status
6. **Handle failures**: If a worker fails, read its status and decide whether to retry or reassign
7. **Collect results**: When workers complete, merge their work (e.g., merge branches)
8. **Clean up**: Remove worktrees when done:
   ```bash
   git worktree remove /path/to/worktree-name
   ```

## Worker Prompt Best Practices

When creating workers, give them **self-contained** prompts that include:
- Exactly what file(s) to work on
- What the expected outcome is
- Any constraints or patterns to follow
- The working directory they should use

Example:
```
create_worker_mission(
  title: "Prove Storage.get_slot lemma",
  model_override: "claude-sonnet-4-5-20250929",
  working_directory: "/workspace/worktree-storage",
  prompt: "In the file Proofs/Storage/GetSlot.lean, replace all `sorry` with valid Lean 4 proofs. Run `lake build` to verify compilation. Do not modify any other files."
)
```

## State Management

Maintain a task tracker file (e.g., `orchestrator-state.json`) to persist your plan:
```json
{
  "tasks": [
    {"id": "task-1", "file": "Foo.lean", "worker_mission_id": "uuid", "status": "in_progress"},
    {"id": "task-2", "file": "Bar.lean", "worker_mission_id": null, "status": "pending"}
  ]
}
```

Update this file as workers complete or fail so you can recover from interruptions.

## Concurrency Limits

Be mindful of the workspace's `max_parallel_missions` setting. Don't spawn more workers
than the system can handle. Start with a small batch, monitor, then scale up.
