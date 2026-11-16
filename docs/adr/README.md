# Architecture decision records

One file per decision: `NNNN-short-title.md`, numbered in order. The files in this folder are the
index.

## Numbers never move

A pull request keeps the ADR number it was given until it merges or closes, so its author never
has to renumber because another PR merged first.

* **Taking a number:** use the next number above every ADR in this folder **and** every number in
  the table below. Add your number to the table in the same PR (or ask a maintainer to reserve it
  for you before you start).
* **Reserved numbers are skipped.** No other PR may use a reserved number, even if a lower number
  looks free on `main`. Gaps in the sequence are expected.
* **When a PR merges,** its row leaves the table (the ADR file now holds the number). **When a PR
  closes without merging,** its row leaves the table and the number stays unused.
* Maintainers keep this table current for open pull requests from anyone, including outside
  contributors whose PR already contains an ADR: that PR's number is reserved as it is.

## Reserved for open pull requests

| Number | Pull request | Topic |
|---|---|---|
| 0037 | #38 (outside contribution) | Interface languages |
| 0096 | #336 | Raw brush, pen pressure |
| 0097 | #354 | Fill and stroke alpha |
| 0098 | feat/studio-chrome (local) | Canvas turn with Shift+Space, zoom drag with Ctrl+Space |
| 0099 | feat/studio-chrome (local) | Studio chrome and 2kool account files |
| 0100 | feat/studio-chrome (local) | Vertical X-sheet on the same timeline |
| 0101 | feat/studio-chrome (local) | Blender-style grab, rotate and scale |
| 0102 | feat/studio-chrome (local) | Pen pressure read in one place |
| 0103 | feat/studio-chrome (local) | Stroke fill alpha on the colour picker |
