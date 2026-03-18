---
description: Agile Autonomous Loop (3-Agent Structure) for rapid development.
---

# 3-Agent Autonomous Light Loop

This workflow is designed for speed and agility, suitable for minor features, refactors, and bug fixes without heavy audit requirements.

## Workflow Steps

### 1. Planning & Design (PM/Architect)
- **PM**: Create/Update `task.md`.
- **Architect**: Create/Update `implementation_plan.md`.
- **Action**: Switch to `Mode: PLANNING`.

### 2. Implementation (Dev)
// turbo
- **Programmer**: Implement code according to the plan.
- **Action**: Switch to `Mode: EXECUTION`. Run `cargo build`.

### 3. Verification & Release (QA + PM)
// turbo
- **QA**: Run `cargo test` and verify fix/feature.
- **Action**: Switch to `Mode: VERIFICATION`. Create `walkthrough.md` and notify user.

## Role Transitions
A single agent context can handle all roles sequentially. Focus is on reaching a "Green" state as quickly as possible while maintaining code quality.
