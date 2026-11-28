# Legacy `.fla` fixtures

## `flash8-tester.fla`

A small test file made in Flash 8 and saved as a Flash 8 binary `.fla`: one scene of 24
frames at 12 fps, 550 × 400, with seven layers (a guide layer among them) and three library
symbols. The button has Up, Over, Down and Hit frames with static text. The graphic has a
shape tween and is placed on classic tweens, two of them eased. The movie clip holds static
text.

* **Maker:** the GitHub user goodtestingtube, since renamed shitgasring. They drew it in
  their own copy of Flash 8.
* **Source:** `anicraft tester no wave.zip`, attached to their
  [comment of 2026-11-24](https://github.com/storytold/anicraft/pull/478#issuecomment-7147201434)
  on storytold/anicraft#478. The zip holds one file, `anicraft tester no wave.fla`, committed
  here unchanged under a new name.
  sha256 `e86ebf63d335f96ce69796f7a86b3cb60f65e1f8e41878a0a9ffc9a0410f2ff4`.
* **Licence:** MIT. Their words in that comment: "All files I've sent in .FLA format are MIT
  licensed". The licence text is in
  [`licenses/fixtures/LICENSE-MIT-flash8-tester.txt`](../../../../licenses/fixtures/LICENSE-MIT-flash8-tester.txt).
* **Contents checked before committing:**
  * It embeds no sound. The maker made a second copy with a test tone they generated in
    Audacity; this copy is the one without the tone.
  * It embeds no bitmaps, no fonts and no other media: its text uses Flash's device fonts
    `_sans` and `_serif`.
  * It holds no personal data: no paths and no names. Its publish settings are Flash's
    defaults (`Untitled-1.*`).

Not committed: the maker's CS6 XFL copies of this file and their PNG export of its frames.
Their licence wasn't stated.
