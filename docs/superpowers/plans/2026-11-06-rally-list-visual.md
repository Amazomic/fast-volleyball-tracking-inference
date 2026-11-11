# Rally list visual refresh Implementation Plan

> **For agentic workers:** Use executing-plans to implement this plan in the current session.

**Goal:** Apply the approved compact card design to the detected rally sidebar.

**Architecture:** Keep existing HTML, data attributes and selection handlers. Replace the scoped CSS presentation in result-media.css.

**Tech Stack:** CSS, existing Node regression checks.

**Spec:** Approved design in this chat: unified compact cards, two-row heading, subdued borders, inline montage selection, clear active states and thin scrollbar.

## Global Constraints

No code comments. Preserve unrelated workspace edits. No commits requested.

## Review Focus

Narrow sidebar, long names, checked selection, keyboard focus, active playback.

### Task 1: Refresh scoped sidebar styles

- [x] Replace fixed-height cards with a unified card containing the playback area and a compact montage footer.
- [x] Stack title and sorting control, preserve readable metadata and ellipsis for long names.
- [x] Verify stylesheet parsing, relevant existing Node tests and inspect available browser preview.

Validation: results_rally_playlist.cjs and montage_rally_player.cjs passed; git diff --check passed. Browser visual inspection was unavailable; narrow-width appearance remains to be reviewed in the live UI.
