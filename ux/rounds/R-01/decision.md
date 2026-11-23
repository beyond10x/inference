# R-01 decision: collapse the Concepts, Guides and Examples categories of the sidebar by default?

Finding: UXF-004. Target metric: 1440x900/glance_cost.p95 of find-error-kinds, find-limitations and
find-realtime-model-variable, now 3, expected 1; their screen_only.actions may rise by 1.
Yes: `collapsed: true` for Concepts, Guides and Examples in `website/sidebars.ts`; the active page's
category opens itself. No shot taken in R-01.
No: every category stays expanded; Reference and Project status need a sidebar scroll at 1440x900.
Default if unanswered by the next round: no, because the change trades a scroll for a click and the
screen-only user measures both; R-02 measures the yes branch before it is taken.
Answer: open
