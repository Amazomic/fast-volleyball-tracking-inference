# Player photo diversity Implementation Plan

**Goal:** Offer new distinct photos on roster cards and an independent reversible similar-photo filter in saved photos.

**Architecture:** Shared conservative thumbnail comparison for candidate selection and album filtering. Existing manual save endpoint remains responsible for persistence; filtering is read-only.

**Constraints:** No code comments; preserve rejected photos and confirmed-track boundary; guard asynchronous UI against player/video changes; preserve unrelated edits.

- [x] Test duplicate selection, distinct poses, missing files, rejected sources and non-destructive filtering.
- [x] Add shared comparison, pending-photo endpoint and optional album filtering.
- [x] Add roster save affordance and album filter toggle with async context guards.
- [x] Run focused Python and Node regression tests, syntax and whitespace checks.

Validation: 17 Python checks passed in the existing WSL environment with OpenCV 5.0.0, including real JPEG comparison and reference matching. Roster, gallery filtering, assignment and album Node checks passed. Syntax and diff whitespace checks passed. Browser visual verification was unavailable.

Implementation decision: assignment performs album ownership reconciliation only; saving new reference photos is explicit. Similar-photo filtering is reversible and never deletes images.
