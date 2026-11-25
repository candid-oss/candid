# UI Guidelines (Mac)

[中文](design-system.md) · **English**

**Status**: Partly implemented. The design system, the style check and the specimen page are implemented. "Reduce motion" is handled only on a few pages.

This document defines the interface design logic and usage rules of the Candid Mac client: how to choose when a kind of UI problem comes up, which component to use, which practices are not allowed; and the interaction of the home timeline, list-plus-detail pages and the menu bar panel. It does not define the data and interfaces behind the UI (see the [Query spec](query.en.md) and the [Architecture overview](../architecture.md)). It does not copy concrete values. Values live only in code.

<a id="overview"></a>

## 1. Overview

- **Values**: font sizes, spacing, corner radii, background shades, colors and motion parameters are defined only in code under `apps/macos/Sources/Candid/DesignSystem/` (the split of files is in section 14). This document gives only names and uses, not values.
- **Specimen**: start with `--design-specimen` and the top bar gets a palette button that opens a specimen page rendered by the real components. The top right switches between the three styles.
- The visual treatment of new features reuses the existing components, styles and themes and does not add a separate visual design. The two-row timeline, the connections page and common actions are built to the interface.
- Schedules and reminders are managed by the agent. "Today" (「今天」) only shows records, offers "Ask AI" (「问 AI」) and proposal approval, and has no manual create, edit or date forms.
- Layout of model settings: sub-pages under the same title row, four statistics, source cards with a call chart, aligned model-role rows, and a selection panel that is searchable and filterable by capability. Model selection follows "expand in place" from section 2. (Unverified.)

<a id="design-logic"></a>

## 2. Design logic

1. **One window.** All pages float on the same background. There are no sidebar blocks or top-bar blocks, and no stacked cards to mark off regions.
2. **Hierarchy comes from background shade and spacing.** At most one more layer of panel sits on the window background. To separate content, use spacing first, then thin dividers, and a light background shade last. Only selection and focus get an outline.
3. **Expand in place.** Adding, editing, confirming and prompt editing all expand under the row that triggers them. A dialog appears only when the system must take over: permission requests, file picking, the color panel.
4. **One title per page.** A page has only one title-level heading. Under a section heading there is no description by default. A description appears only once, where it affects a decision.
5. **One primary action per region.** A region has at most one solid button. The other actions are quiet buttons.
6. **States have fixed wording.** The same state uses the same label and the same color on every page.
7. **Components first.** Pages only combine components and design parameters. They do not pick font sizes, corner radii or background shades themselves. When a new look is needed, first add a component or variant in `DesignSystem/` and put it on the specimen page.

<a id="styles"></a>

## 3. Styles

The three styles share all parameters and components. They differ only in the background and in "how a panel is set apart from the background".

| Style | Background | How a panel is set apart from the background | Floating panel |
|---|---|---|---|
| Ambient (氛围) | Soft light that changes color with content, with an adjustable frosted-glass layer on top | Light background shade | System material |
| Frosted glass (毛玻璃) | The desktop behind the window is blurred, with an even tint on top (neutral or chosen) | Light background shade | System material |
| Solid (纯色) | One flat color (neutral, or with a chosen color mixed in lightly) | White panel (a lightened gray in dark mode) with a soft shadow. A panel inside a panel uses a light background shade and does not raise a second layer. | Solid panel with shadow |

- The style is saved in user settings and passed to all components through an environment value. Components choose how to draw themselves from the style. Pages do not check the style.
- When a new component needs a "panel feel", use `panelSurface`. When it floats above content, use `floatingSurface`. Do not draw a white background or a material directly.

<a id="type"></a>

## 4. Type

Seven levels, from large to small: display, title, section, headline, body, secondary, caption.

| Level | Used for |
|---|---|
| display | Clocks and large numbers |
| title | The one title of a page |
| section | Sections within a page |
| headline | Row titles, navigation items, card titles |
| body | Body text, fields, buttons |
| secondary | In-row descriptions, hints at decision points, meta information |
| caption | Labels, counts, chart axes |

- Set text with `typography(_:)` and change only the weight when needed. Do not write concrete font sizes.
- Numbers that change in place (durations, counts, clocks) use monospaced digits.
- Addresses, code and model IDs use a monospaced font.
- Color levels use only three steps: primary, secondary, tertiary.

<a id="color"></a>

## 5. Color

Color only carries meaning. Decoration is left to the background and the app's own colors.

| Meaning | Use |
|---|---|
| Interactive color | Primary button, selection outline, focus glow, and labels such as "Modified" that show a user change. Follows the system accent color. There is no separate brand color. |
| Done, warning, error | Used only on status dots and in-place hints. |
| Recording | Only for capture in progress. Not used for emphasis, except for dangerous actions such as delete. |
| Destination | Three kinds: on this device, self-hosted, cloud. Each has a fixed color, shown only on destination labels and the matching icons. |
| AI gradient | Only marks AI pages and AI-generated content. Not used on ordinary controls. |
| App color | Extracted from the app icon and used in the timeline and for episodes. The same app always gets the same color. (Unverified: the extraction logic is not inside `DesignSystem/`.) |

<a id="surfaces"></a>

## 6. Surfaces, corner radii and spacing

- **Four background shades**: wash (panels expanded in place), fill (fields and idle options), hover (hovering), selected (selected, pressed). All are low-opacity versions of the main text color, so they work in both light and dark mode.
- **Thin dividers** only separate list rows. Groups are separated by spacing.
- **Four corner radius levels**: small (fields, in-row thumbnails), medium (option cards, edit boxes), large (expanded panels, floating cards), card (media cards, large previews). Anything fully rounded at both ends uses a capsule shape. All corners are continuous curves.
- **Three control heights**: chip (labels, small capsules), button (options, quiet buttons, primary buttons), field (fields, round buttons in the top bar). This keeps controls in one row aligned.
- **Spacing** is in multiples of 4: xs between elements in a row, row between rows, l between groups, page for the left and right page margins.
- Scrolling content fades out before the top bar (`fadesUnderChrome`) and must not pass under the navigation text.

<a id="components"></a>

## 7. Components and usage

| Need | Use | Do not use |
|---|---|---|
| The primary action of a region | `PrimaryButton`, can be set as the Return default | Two solid buttons in one region |
| Other actions | `QuietButton`, shows a background only on hover | System bordered buttons |
| A set of parallel actions (title, summary, tags…) | `SecondaryButton`: light-background capsule | System white-background buttons |
| Irreversible actions such as delete (in their own row) | `ConfirmButton`, confirmed a second time in place | System confirmation dialogs |
| Irreversible actions such as delete (started from a menu or card) | `confirmCapsule`: a confirmation bar floating under the top bar that states the consequence | System confirmation dialogs |
| Small actions in the middle of text (Ask AI, expand, undo) | `ChipButton`: light-background capsule, clearly clickable even at rest | Text buttons that look like body text at rest |
| AI actions that may take a while (review, pre-meeting points, run a skill) | `RunButton`: after a click it becomes a spinner plus a description plus "Cancel" (「取消」), and cancel really stops the run | No feedback after the click, cannot be stopped midway |
| Top-bar icon actions | `ChromeButton` | Hand-drawn square icon buttons |
| A "more" menu in the top bar or next to a title | `ChromeMenu` | System borderless menu (`menuStyle(.borderlessButton)`, blocked by the style check) |
| Minor text menus (speed, add fallback) | `QuietMenu` | Same as above |
| Switch between different views of the same content | `PillSwitcher` | System segmented control |
| Setting a value (up to about five options) | `CapsuleChoice` | System segmented control, radio buttons |
| Setting a value (more options) | `MenuChoice`: a light-background capsule showing the current value and a small arrow, with a system menu for the list | System pop-up button, `Picker` |
| Open a small group of actions (generate draft → title/summary…) | `MenuButton`, looks like `MenuChoice` | A bordered `Menu` |
| On and off | `SwitchRow` (system switch) | Checkbox |
| Several independent switches on the same object | `ToggleChip` (such as an app's screenshot, text, exclude) | A row of checkboxes or switches |
| Integer stepping | `StepperControl`: value text plus a small stepper | A stepper alone with no visible value |
| One setting | `SettingRow`: title on the left (add one sentence if there are consequences), control on the right | System grouped `Form`, `LabeledContent` |
| Settings pages and groups | `SettingsPage` (one section-level title), `SettingsGroup` (headline-level group name, can carry an action on the right) | System `Section` |
| Rarely used advanced items | `FoldRow`, expands in place | `DisclosureGroup` |
| Text input | `QuietField`, title on top | Bordered text box |
| In-page search | `SearchField` | Bordered text box |
| Long text, prompts | `PromptEditor`, inside an `InlinePanel` | Editing in a dialog |
| Content expanded under a row | `InlinePanel` | sheet, popover |
| Dropdown content of a top-bar button | popover, using only components from this table inside | Hand-drawn floating layers |
| Choosing a date (home title) | `DateJumperView`: a popover month calendar. The amount of records is shown by shade of the accent color, empty days are faded, today is outlined, the current day is filled | System date picker |
| A set of selectable starting points | `PresetTile` grid | Long descriptions in a dropdown menu |
| Default or modified | `StateTag` | Custom text |
| General label | `Tag`: gray states a fact, color expresses meaning | Custom capsule |
| Where content goes | `DestinationTag` | Custom colors |
| Source identity, seven-day calls and proportion | `SourceBadge`, `ActivityBars`, `ProportionMeter`, using only real data | Made-up online status, cost or call curves |
| Icon actions in a settings row | `IconChipButton`, with a readable name | A page assembling its own set of buttons |
| Connection or capture status | `StatusDot` | Color only, with no text |
| An in-place problem or hint | `InlineMessage` | Banner |
| Something that happened elsewhere | `NoticeCapsule` (floating under the top bar) | System notification, dialog |
| Hover card, preview | `floatingSurface` | Hand-drawn shadow and border |
| Section | `SectionHeader` | Bold body text |

<a id="feedback"></a>

## 8. Feedback and empty states

- Errors are written where the problem is: under the field, in the row. The floating hint at the top is only for things that happen elsewhere.
- Error text says what happened and what to do next, for example "The service returned 401, an access token is required" (「服务返回 401，需要访问令牌」).
- An empty state says what is missing and gives the next-step button directly. No illustrations and no slogans.
- When authorization is needed, the "Authorize" button sits next to the matching capability and states what it is for.
- While recording there must be continuous visible motion feedback (a waveform or a light orb). Red is kept for active recording only.

<a id="motion"></a>

## 9. Motion

- Use only the named animations in `Motion`: spring for pages and layout, snappy for hover, selection and label changes, bouncy for user-initiated switches and landing, magnify for following the pointer, ambient for background color changes.
- All are interruptible springs, and a new gesture can always interrupt a running animation. Only values that follow a live signal (levels, playback progress) use constant speed.
- Content enters with `Motion.enter` (fade in and move up a little). Expanding under a row uses `Motion.unfold`.
- When the system has "Reduce motion" on, springs and displacements become fades. (Partly implemented: at present only `SettingsHomeView` reads this system setting and turns animations off. It has not been changed to fades, and there is no global handling.)

<a id="accessibility"></a>

## 10. Accessibility

- Icon buttons all have an accessibility label. Purely decorative elements are hidden.
- Selected state adds `isSelected`. A combined row is merged into one accessibility element.
- Contrast between text and background must be readable in all three styles, in light and dark mode. Check the specimen page at acceptance.

<a id="checks"></a>

## 11. Checks and acceptance

- **Automatic check**: `apps/macos/script/test.sh` first runs the style check (`script/check_style.sh`). The test fails if, outside `DesignSystem/`, there are hard-coded font sizes, hard-coded corner radius values, custom background opacities, or system controls that section 7 replaces (grouped `Form`, `GroupBox`, confirmation dialogs, checkboxes, segmented controls, `DisclosureGroup`, system dividers and so on). Geometry that truly belongs to the picture content (such as timeline ticks, or the schematic window in a screenshot) is marked on the same line with `// style-exempt: reason`, and only for drawing content, not for controls.
- **Specimen**: update the specimen page whenever a component is added or changed.
- **Screenshot acceptance**: after a UI change, take screenshots of the relevant pages in a test copy. Look at both the Ambient and Solid styles, in light and dark mode. The blur behind the window in Frosted glass is composited by the window server, so the test copy's screenshot cannot capture it. It must be confirmed in a real window.

<a id="timeline-lanes"></a>

## 12. Home timeline (multi-lane)

The home timeline is several lanes on one time axis. Information order: the stage above answers "what is being done now", the lanes below answer "what happened this day", and the two share one playhead.

- **Stage**: text on the left (app, title, time, and the recordings, AI instructions and screenshots of the same period, each row clickable), and one current screenshot on the right. Earlier and later episodes only show faded at the two sides. The stage shows only one image, and piles of screenshots are left to the screenshot lane. When the timeline is stretched so the stage is less than about 190pt, the stage collapses into one row.
- **Lanes**: from top to bottom, screen, screenshots, AI, recordings. Lanes such as calendar are added when new sources are connected. Lane names are only labels (hover shows a description), and a click does nothing. Which lanes are shown is ticked in the right-click menu "Shown lanes" (「显示的轨道」), and the screen lane is always shown. Height is adjusted by one value only (drag the top bar, snapping to compact, standard and expanded), and all lanes change proportionally. The user does not adjust them one by one.
- **Level of detail follows zoom**: in the screen lane, icons float above thin color bars and fold into "+N" when close together. Switches to another app within 3 minutes in the middle of a same-app episode fold into that episode and draw a gap. Consecutive episodes narrower than 16pt merge into one color bar, and a click zooms to that stretch. In the screenshot lane, when short it is color dots and when tall it is a film strip. Film cells are grouped by fixed clock spans (5, 10, 30 minutes… the shortest span that fits is chosen by zoom). When scrolling, the whole strip pans, does not regroup and does not change images. It rearranges only when zoom crosses a step. Each cell shows the first screenshot of that span by default, with the count. Hovering and moving left and right previews the screenshots one by one, and when the image changes the old one stays until the new one is ready. In the AI lane, when short there are only instruction dots and work bars, and when tall the instruction text is shown.
- **Screen-lane titles**: only visible icons get a title. The text stops at the current episode, the next visible icon or the edge of the view. If there is not enough room it is shortened or not shown. Neighbors folded into "+N" no longer show their own titles. The full name is provided by the icon's accessibility name and the episode details.
- **Open**: details expand in place at the stage position, the timeline stays below with the item highlighted. Esc closes, and ←→/↑↓ switch within the episode. In episode details, the left column is a vertically scrollable list of moments (thumbnail, time, window; a switch to another app in the middle also gets a row). The big image in the middle fills the whole window when clicked (covering the navigation; pinch or double-click to zoom, drag to pan, ←→ for previous/next image, Esc or Space to close). The right column holds only "What this stretch is about" (「这段在做什么」), "At the same time" (「同一时间」) (AI instructions, recordings) and a collapsed screen text. Recognition and record information goes into "Technical details" (「技术细节」).
- **AI lane**: draws only moment dots. The work duration after a dot fades in only on hover or when one is opened. The hover card shows the source (Codex / Claude Code) · project folder · session name (session title, or the beginning of the first instruction of that session that day if missing), the first two lines of the original words, the time, the work duration, and which instruction of the session that day it is. AI instruction details prefer the saved original words, and on open they read the reply after it live from the session file (not stored in the timeline), up to the next instruction. If it cannot be read, only one line of hint is added. The title bar has ‹ › (or ←→) to move to the previous/next instruction of the day by time, and several instructions stacked in one place are also viewed one by one this way. The "Open source" menu lists the jumps that can be done: continue the session in a terminal (`claude --resume` / `codex resume`), copy that command, show the session file in Finder, open the project folder.
- **Right click**: episodes, merged segments, screenshots, AI instructions, recordings, blank space and lane names each have a menu. It only pops up a menu and does not change the selection, the playhead or the view. Destructive actions still go through `confirmCapsule`.
- **Geometry rules** (merging, folding, bucketing, height) are implemented in `TimelineLaneLayout` and have unit tests. Values are defined only there and in `DesignSystem/`.

<a id="timeline-interaction"></a>

### 12.1 Interaction rules

Principle: looking and locating are separate; one action gives one result and takes effect at once; a view that was moved by hand is not pulled back automatically; new data does not interrupt an operation.

| Action | Result | Does not happen |
|---|---|---|
| Two-finger scroll, mouse wheel | The view pans, following the hand | The playhead and stage do not move, no bounce-back |
| Pinch, ⌘+wheel | Zoom centered on the pointer | The playhead does not move |
| Click blank lane or ruler | The playhead moves to that moment | Does not open details |
| Press and drag | Drag the playhead, auto-pan at the edge | No rearranging or flicker during the drag |
| Click an episode, recording or screenshot | Locate: the playhead goes to the item and the stage shows it; a merged block zooms in | Does not open details directly |
| Click an already located item again, double-click, click the stage card, Return | Open details; when details are already open, clicking a lane item switches the content directly | Does not show something else first and then jump |
| Click an AI instruction dot | Expands in place | — |
| The playhead leaves the view | A "◀ time" label appears at the edge, click it to bring it back | No automatic pull-back |
| "Back to now" (「回到现在」), jump, arrow keys, play | The view follows the playhead | — |

- **Follow now**: on today, when not manually located, the playhead moves with time, and the view pans only when the playhead nears the right edge. Locating or dragging leaves this mode and "Back to now" appears.
- **New data**: new records that arrive during drag, scroll or pinch are merged about 0.3 seconds after you stop. The timeline is built in the background, about once every 0.5 seconds at most, and merging does not move the view.

<a id="menu-bar-panel"></a>

### 12.2 Menu bar panel

Clicking the menu bar icon opens and closes the panel. Clicking outside the panel, switching to another app or pressing Esc closes it at once. "Open main window" (「打开主窗口」) in the panel closes it after running, and toggle-type actions leave it open. It is implemented as `NSStatusItem` + a transient `NSPopover` (`StatusItemController`).

<a id="list-detail"></a>

## 13. List-plus-detail pages

The connections page and the settings page have the same structure: on the left a list grouped by category (icon, name, status dot), on the right a detail panel for the selected item (one sentence of description, key numbers, authorization, the item's own settings). Planned items are folded at the bottom of the list. Adding a source adds only one row in the list, not a new card block. The AI page is organized in two columns: "Automatic processing / Workflows / People and voiceprints" (「自动处理 / 流程 / 人物与声纹」) and "Manual generation / Recent tasks" (「手动生成 / 最近任务」). Planned abilities shrink into one row at the bottom.

<a id="code"></a>

## 14. Where it is implemented

`apps/macos/Sources/Candid/DesignSystem/`:

| File | Responsible for |
|---|---|
| `Tokens.swift` | `TypeScale`, `Space`, `Radius`, `ControlHeight`, `Surface`, `Palette` — sections 4, 5, 6 |
| `VisualStyle.swift` | The three styles (Ambient, Frosted glass, Solid), the environment value, `WindowBackdrop`, `BehindWindowBlur` — section 3 |
| `Surfaces.swift` | Panel and floating surfaces, `fadesUnderChrome`, `InlinePanel` and the settings page components (`SettingsPage`, `SettingsGroup`, `SettingRow`, `FoldRow`, `SectionHeader`) — sections 3, 6, 7 |
| `Controls.swift` | The button, menu, choice, field and stepper components in the table of section 7 (`PrimaryButton` through `IconChipButton`, except `SwitchRow`) |
| `Indicators.swift` | Labels, status dots, hints and confirmation bars, source and usage indicators — sections 7, 8 |
| `Motion.swift` | The named animations spring, snappy, bouncy, magnify, ambient, and the `enter` and `unfold` transitions — section 9 |

Listed as components in this document but not inside `DesignSystem/`: `SwitchRow` (`Views/Desktop/DesktopSettingsView.swift`), `PromptEditor` (`Views/Home/PromptEditor.swift`), `PresetTile` (`Views/Home/ModelSourcesView.swift`), `DateJumperView` (`Views/Home/DateJumper.swift`). These four do not match "Components first" (point 7 of section 2) and have not yet been moved into `DesignSystem/`.

| Content | Code | Test or check |
|---|---|---|
| Specimen page (section 1) | `Views/DesignSpecimen/DesignSpecimenView.swift`, added by `HomeRootView` under the `--design-specimen` launch argument | Manual acceptance |
| Style check (section 11) | `script/check_style.sh`, called first by `script/test.sh` | The script itself |
| UI text localization | `script/localize_sources.py`: `--check` checks that every literal containing Chinese is wrapped in `L(...)` and has an English entry; English is in `Resources/Localizable.en.json` | `test.sh` calls `--check`; `Tests/CandidLocalizationTests` |
| Timeline geometry (section 12) | `Views/Home/TimelineLaneLayout.swift` | `Tests/DesktopSessionTests/TimelineLaneLayoutTests.swift` |
| Menu bar panel (section 12.2) | `StatusItemController` in `CandidApp.swift` | None (unverified) |

How the rules of `check_style.sh` map to this document (it scans only `Sources/Candid/`, excluding `DesignSystem/` and lines with `// style-exempt:`):

| Pattern checked | Matching rule |
|---|---|
| `.font(.system(size:…))`, `.font(.largeTitle/.title/…/.caption2)` | Section 4: use `typography(_:)`, do not write font sizes |
| `cornerRadius: <number>` | Section 6: corner radii use the four `Radius` levels |
| `Color.primary.opacity(` | Section 6: backgrounds use the four `Surface` shades |
| `Form {`, `GroupBox`, `DisclosureGroup`, `Divider()` | Section 7: `SettingRow`/`SettingsGroup`, `FoldRow`, thin dividers |
| `confirmationDialog`, `.alert(`, `.sheet(` | Section 2 point 3 and section 7: expand in place, `ConfirmButton`, `confirmCapsule` |
| `buttonStyle(.bordered`, `toggleStyle(.checkbox`, `pickerStyle(.segmented`, `menuStyle(.borderlessButton)` | Section 7: button, switch, choice and menu components |
| `ContentUnavailableView` | Section 8: an empty state says what is missing and gives a next step |

Without an automatic check: sections 5, 8, 9, 10, 12 and 13.

<a id="open"></a>

## 15. Not implemented and open items

| Item | Status | Note |
|---|---|---|
| Reduce motion | Partly implemented | See section 9 |
| Move `SwitchRow`, `PromptEditor`, `PresetTile`, `DateJumperView` into `DesignSystem/` | Not implemented | See section 14 |
| Extraction and stability of app colors | Unverified | Section 5; not found inside `DesignSystem/` |
| Layout of model settings | Unverified | Last item of section 1, not checked one by one |
| Tests for the menu bar panel | Unverified | Section 12.2 |
| Interactions in sections 12, 12.1 and 13 | Unverified | Only confirmed that the geometry layer has unit tests |

<a id="sources"></a>

## 16. Sources

All content was moved from the old `docs/specs/design-system-v1.md`. For how the old sections map to this document, see the move-mapping page in that file.
