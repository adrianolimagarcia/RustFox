# ADR 0014: Plan lookup uses an active pointer, not `default.json`

## Status
Accepted

## Date
2026-10-04

## Context
`plan_create` writes `.plans/<title>.json`. `plan_update` and `plan_view` do not take `title` in the schema, so the implementation falls back to `default.json`, which is never created. A plan can be created but not viewed or updated. `notes` is on the update schema, but create never writes a notes array, so notes are dropped. Titles are used raw as filenames. A missing file surfaces as a raw OS error.

This is not a 1.0.4 release gate. Product locked the behavior on 2026-10-04 in the RustFox room.

## Decision
- One sandbox (`allowed_directory`) has one active plan. The active plan is a pointer, not the newest file by mtime. `plan_create` and a successful `plan_update` move the pointer. A failed update does not. `plan_view` does not.
- `plan_view` and `plan_update` take an optional `title`. No title means the active plan. A title means that plan.
- `plan_create` initializes a notes array the same length as steps, empty strings. `plan_update` writes `notes` onto that step.
- Reject a title that is empty or only whitespace, contains `/` or `\`, or has a path segment `..`. Do not rewrite it. `:` , spaces, and CJK stay valid.
- Creating the same title again overwrites that file and moves the pointer.
- If the plan is missing, or `step_id` is out of range, return a clear error. A missing plan lists existing titles. Do not return a raw OS error.
- `plan_view` returns JSON that includes notes. The Telegram tool bubble does not show notes.

## Consequences
Two chats that share a sandbox share the pointer. A title that collides with the pointer file name must not be possible; the pointer is not a `<title>.json` plan file.
