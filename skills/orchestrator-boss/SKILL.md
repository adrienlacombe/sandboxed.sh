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
- **create_worktree**: Create a git worktree for file-level isolation
- **remove_worktree**: Remove a git worktree when done
- **wait_for_worker**: Block until a worker completes/fails (instead of polling)

## Workflow

1. **Analyze the task**: Break the work into independent, parallelizable units
2. **Plan workers**: Decide how many workers you need and what each will do
3. **Create git worktrees**: Use `create_worktree` to give each worker an isolated directory:
   ```
   create_worktree(path: "/workspaces/xxx/worktree-1", branch: "worker/task-1", base: "main")
   ```
4. **Spawn workers**: Use `create_worker_mission` with `working_directory` set to the worktree:
   ```
   create_worker_mission(
     title: "Prove Storage.get_slot lemma",
     model_override: "claude-sonnet-4-5-20250929",
     working_directory: "/workspaces/xxx/worktree-1",
     prompt: "Your task: ..."
   )
   ```
5. **Wait for workers**: Use `wait_for_worker` to block until each worker finishes — no need to poll
6. **Handle failures**: If a worker fails, read its status and decide whether to retry or reassign
7. **Collect results**: When workers complete, merge their branches
8. **Clean up**: Use `remove_worktree` for each finished worktree

## Worker Prompt Best Practices

When creating workers, give them **self-contained** prompts that include:
- Exactly what file(s) to work on
- What the expected outcome is
- Any constraints or patterns to follow
- Build/test commands to verify their work

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
