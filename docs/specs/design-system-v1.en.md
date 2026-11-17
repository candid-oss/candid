# Design system v1 (Mac)

[中文](design-system-v1.md) · **English**

How the Candid Mac app decides UI questions: which component to use for a given need, and what is not allowed.

- **Values**: type sizes, spacing, radii, surface tints, colors and motion parameters are defined only in code, in [`DesignSystem/`](../../apps/macos/Sources/Candid/DesignSystem/). This page names them and says what they are for; it never copies values.
- **Specimen**: launch with `--design-specimen` to get a palette button in the top bar; it opens a page rendered from the real components, with the three styles switchable at the top right.

> New features reuse the existing components, styles and visual language rather than a new visual design. The home timeline uses one time axis with several lanes (section 11). Agenda and reminders are managed by the agent: "Today" only shows records, offers "Ask AI" and proposal approval, and has no manual create/edit forms.

## 1. Principles

1. **One window**: every page floats on the same background. No sidebar boxes, no top-bar boxes, no stacked cards to separate areas.
2. **Hierarchy from tint and space**: at most one panel layer above the window background. Separate content first with space, then hairlines, and only then a faint tint. Only selection and focus get an outline.
3. **Unfold in place**: adding, editing, confirming and prompt editing unfold under the row that triggered them. Modal windows only where the system must take over: permission prompts, file pickers, the color panel.
4. **One title per page**: a page has a single title-level heading. Section headings have no description by default; an explanation appears once, only where it affects a decision.
5. **One primary action per area**: at most one filled button per area; everything else is a quiet button.
6. **Fixed words for states**: a given state uses the same label and color on every page.
7. **Components first**: pages compose components and design tokens and never pick their own sizes, radii or tints. A new look starts as a component or variant in `DesignSystem/` and goes into the specimen.

## 2. Styles

The three styles share every token and component and differ only in the background and in how panels separate from it.

| Style | Background | Panel vs background | Floating panels |
|---|---|---|---|
| Ambient | Soft light that follows the content, under an adjustable frosted layer | Faint tint | System material |
| Frosted | The desktop behind the window, blurred, under an even tint (neutral or chosen) | Faint tint | System material |
| Solid | One flat color (neutral, or a chosen color lightly mixed in) | White panel (lifted gray in dark mode) with a soft shadow; panels inside panels use a faint tint instead of a second lift | Solid panel with shadow |

- The style lives in user settings and reaches every component through the environment; components decide how to draw, pages never check the style.
- A new component that needs a panel uses `panelSurface`; one that floats above content uses `floatingSurface`. Never draw white backgrounds or materials directly.

## 3. Type

Seven levels, largest first: display, title, section, headline, body, secondary, caption.

| Level | For |
|---|---|
| display | Clocks and large numbers |
| title | The page's only title |
| section | Sections within a page |
| headline | Row titles, navigation items, card titles |
| body | Body text, fields, buttons |
| secondary | Inline explanations, hints where a decision is made, metadata |
| caption | Tags, counts, axes |

- Set text with `typography(_:)`; change only the weight when needed, never a size.
- Numbers that change in place (durations, counts, clocks) use monospaced digits.
- Addresses, code and model IDs use a monospaced face.
- Only three text color levels: primary, secondary, tertiary.

## 4. Color

Color only carries meaning; decoration comes from the background and from the apps' own colors.

| Meaning | Use |
|---|---|
| Interaction | Primary buttons, selection outlines, focus rings, "modified" tags. Follows the system accent color; no separate brand color. |
| Done, warning, error | Only in status dots and inline messages. |
| Recording | Only for capture in progress; not used to emphasize anything except destructive actions. |
| Destination | On this Mac, self-hosted, cloud — each with a fixed color, only on destination tags and their icons. |
| AI gradient | Only marks the AI page and AI-generated content, never ordinary controls. |
| App color | Extracted from the app icon, used on the timeline and in episodes; the same app always gets the same color. |

## 5. Surfaces, radii and spacing

- **Four tints**: wash (panels unfolded in place), fill (fields and idle options), hover, selected (selected or pressed). All are low-opacity primary text color, valid in light and dark.
- **Hairlines** only separate list rows; groups are separated by space.
- **Four radii**: small (fields, inline thumbnails), medium (option cards, editors), large (unfolded panels, floating cards), card (media cards, large previews). Fully rounded ends always use a capsule. All corners are continuous curves.
- **Three control heights**: chip (tags, small capsules), button (options, quiet and primary buttons), field (fields, round top-bar buttons), so controls in a row line up.
- **Spacing** in multiples of 4: xs inside a row, row between rows, l between groups, page for page margins.
- Scrolling content fades out before the top bar (`fadesUnderChrome`) and never passes under navigation text.

## 6. Components

| Need | Use | Not |
|---|---|---|
| An area's primary action | `PrimaryButton`, optionally the Return default | Two filled buttons in one area |
| Other actions | `QuietButton`, tinted only on hover | System bordered buttons |
| A set of parallel actions (title, summary, tags…) | `SecondaryButton`: light-tinted capsule | System white buttons |
| Irreversible action, in its own row | `ConfirmButton`, confirms in place | System confirmation dialogs |
| Irreversible action, from a menu or card | `confirmCapsule`: a floating confirmation bar under the top bar that states the consequence | System confirmation dialogs |
| Small actions inside text (ask AI, expand, undo) | `ChipButton`: light capsule, visibly clickable at rest | Text buttons that look like body text |
| AI actions that may take a while (review, briefing, running a skill) | `RunButton`: becomes a spinner, a description and "Cancel", and cancel really stops the run | No feedback, no way to stop |
| Top-bar icon actions | `ChromeButton` | Custom square icon buttons |
| "More" menu in the top bar or next to a title | `ChromeMenu` | System borderless menus (`menuStyle(.borderlessButton)`, blocked by the style check) |
| Secondary text menus (speed, add fallback) | `QuietMenu` | Same as above |
| Switching views of the same content | `PillSwitcher` | System segmented control |
| A setting with about five values or fewer | `CapsuleChoice` | Segmented controls, radio buttons |
| A setting with more values | `MenuChoice`: light capsule with the current value and a chevron; the list is a system menu | Pop-up buttons, `Picker` |
| Opening a small set of actions (draft → title/summary…) | `MenuButton`, styled like `MenuChoice` | Bordered `Menu` |
| On and off | `SwitchRow` (system switch) | Checkboxes |
| Several independent switches for one object | `ToggleChip` (e.g. an app's screenshots, text, exclusion) | A row of checkboxes or switches |
| Integer steps | `StepperControl`: the value plus a small stepper | A stepper without a visible value |
| One setting | `SettingRow`: title on the left (plus one line on the consequence, when there is one), control on the right | System grouped `Form`, `LabeledContent` |
| Settings pages and groups | `SettingsPage` (one section-level title), `SettingsGroup` (headline-level group name, optional trailing action) | System `Section` |
| Rarely used advanced items | `FoldRow`, unfolds in place | `DisclosureGroup` |
| Text input | `QuietField`, title above | Bordered text fields |
| In-page search | `SearchField` | Bordered text fields |
| Long text, prompts | `PromptEditor` inside an `InlinePanel` | Modal editors |
| Content unfolded under a row | `InlinePanel` | Sheets, popovers |
| Dropdown content of a top-bar button | A popover containing only components from this table | Custom overlays |
| Picking a date (home title) | `DateJumperView`: popover month calendar, record volume as accent intensity, empty days faded, today outlined, current day filled | System date picker |
| A set of starting points | `PresetTile` grid | Long descriptions in a dropdown |
| Default or modified | `StateTag` | Custom text |
| General tags | `Tag`: gray states facts, color carries meaning | Custom capsules |
| Content destination | `DestinationTag` | Custom colors |
| Source identity, seven-day calls and proportions | `SourceBadge`, `ActivityBars`, `ProportionMeter`, with real data only | Invented online status, costs or call curves |
| Icon actions in a settings row | `IconChipButton`, with a readable name | Page-specific button sets |
| Connection or capture state | `StatusDot` | Color alone without text |
| A problem or hint in place | `InlineMessage` | Banners |
| Something that happened elsewhere | `NoticeCapsule` (floats under the top bar) | System notifications, alerts |
| Hover cards, previews | `floatingSurface` | Custom shadows and borders |
| Sections | `SectionHeader` | Bold body text |

## 7. Feedback and empty states

- Errors appear where the problem is: under the field, in the row. Only events from elsewhere use the floating notice.
- Error text says what happened and what to do next, e.g. "The service returned 401; an access token is required."
- Empty states say what is missing and offer the next step as a button; no illustrations or slogans.
- When authorization is needed, the "Authorize" button sits next to the capability, with its purpose.
- Recording must have continuously visible live feedback (waveform or orb); red is reserved for recording in progress.

## 8. Motion

- Only the named animations in `Motion`: spring for pages and layout, snappy for hover, selection and tag changes, bouncy for user-initiated switches and settling, magnify for following the pointer, ambient for background color changes.
- All springs are interruptible; a new gesture always interrupts a running animation. Only values that follow a live signal (levels, playback progress) move linearly.
- Content enters with `Motion.enter` (fade in, slight rise); unfolding under a row uses `Motion.unfold`.
- **To do**: with "Reduce motion" enabled, springs and offsets become fades.

## 9. Accessibility

- Every icon button has an accessibility label; purely decorative elements are hidden.
- Selected states add `isSelected`; composite rows merge into one accessibility element.
- Text and background contrast must be readable in all three styles, light and dark; check the specimen during review.

## 10. Checks and review

- **Automated check**: `apps/macos/script/test.sh` first runs the style check (`script/check_style.sh`). The tests fail when code outside `DesignSystem/` uses hard-coded type sizes, hard-coded radii, custom tint opacities, or a system control replaced in section 6 (grouped `Form`, `GroupBox`, confirmation dialogs, checkboxes, segmented controls, `DisclosureGroup`, system dividers…). Geometry that is genuinely content (timeline ticks, a sketched window in a screenshot) is marked on the same line with `// style-exempt: reason` and is used only for drawing content, never for controls.
- **Specimen**: update the specimen page when adding or changing a component.
- **Screenshot review**: after a UI change, capture the affected pages from a test copy in the Ambient and Solid styles, light and dark. The Frosted style's background blur is composited by the window server and does not appear in test-copy screenshots; check it in a real window.

<a id="list-detail"></a>

## 12. List-and-detail pages

Connections and Settings share one structure: a list on the left grouped by category (icon, name, status dot) and a detail panel on the right for the selected item (one line of explanation, key numbers, authorization, the item's own settings). Planned items fold at the bottom of the list. A new source adds one list row, not a new card block. The AI page has two columns: "automatic processing / pipelines / people and voiceprints" and "manual generation / recent tasks"; planned capabilities collapse into one row at the bottom.

<a id="timeline-lanes"></a>

## 11. Home timeline (lanes)

Order of information: the stage on top answers "what is happening at this moment"; the lanes below answer "what happened today". Both share one playhead.

- **Stage**: text on the left (app, title, time, and the recordings, AI prompts and screenshots of the same moment, each clickable) and one current screenshot on the right, with neighbouring episodes fading at both sides. The stage holds a single image; piles of screenshots belong to the screenshot lane. When the timeline is tall enough to leave the stage under about 190 pt, the stage collapses to one line.
- **Lanes**: top to bottom — screen, screenshots, AI, recordings; new sources such as calendars add lanes later. Lane names are labels only (hover for a description), clicking does nothing; which lanes show is chosen in the context menu "Visible lanes", and the screen lane always shows. Height is a single value (drag the top bar; snaps to compact, standard, expanded) and every lane scales proportionally.
- **Level of detail follows zoom**: screen-lane icons float above a thin color bar and fold into "+N" when crowded; switches away from the same app for under 3 minutes fold into one episode with a gap; consecutive episodes narrower than 16 pt merge into one color band, and clicking it zooms into that span. The screenshot lane is color dots when short and a film strip when tall: frames are grouped by fixed clock intervals (5, 10, 30 minutes… the shortest that fits at the current zoom); scrolling moves the whole strip without regrouping or swapping images, and only crossing a zoom step regroups. Each frame shows the first screenshot of its interval with a count; hovering left and right previews them one by one, and the old image stays until the new one is ready. The AI lane shows prompt dots and work bars when short and prompt text when tall.
- **Screen-lane titles**: only visible icons get titles; text stops at the end of the current episode, the next visible icon or the edge of the view, and is truncated or omitted when there is no room. Neighbours folded into "+N" get no separate title; full names come from the icons' accessible names and the episode details.
- **Opening**: details unfold in place of the stage while the timeline stays below with the item highlighted; Esc closes, ←→/↑↓ move within an episode. An episode's detail shows a vertically scrolling list of moments on the left (thumbnail, time, window; a switch to another app midway is its own row), a large image in the middle that fills the whole window on click (covering navigation; pinch or double-click to zoom, drag to pan, ←→ for previous/next, Esc or Space to close), and on the right only "What this was", "At the same time" (AI prompts, recordings) and folded screen text; recognition and record metadata live under "Technical details".
- **AI lane**: draws moments only; the work duration that follows fades in on hover or when one is opened. The hover card shows the source (Codex / Claude Code) · project folder · session name (the session title, or the start of that session's first prompt of the day), the first two lines of the prompt, the time, the work duration, and which prompt of the session it is that day. Prompt details show the saved prompt first and read the following reply live from the session file (never stored in the timeline), up to the next prompt; if it cannot be read, a single line says so. ‹ › in the title bar (or ←→) move to the previous/next prompt of the day, which also steps through prompts stacked at one spot. "Open source" lists the available jumps: continue the session in a terminal (`claude --resume` / `codex resume`), copy that command, reveal the session file in Finder, open the project folder.
- **Context menus**: episodes, merged spans, screenshots, AI prompts, recordings, empty space and lane names each have a menu. A context menu only opens the menu; it never changes the selection, playhead or view. Destructive actions still go through `confirmCapsule`.

<a id="timeline-interaction"></a>

### 11.1 Interaction rules

Principles: looking and positioning are separate; one action, one result, applied immediately; a view moved by hand is never pulled back automatically; new data never interrupts an action.

| Action | Result | Never |
|---|---|---|
| Two-finger scroll, mouse wheel | The view pans with the fingers | The playhead and stage do not move; no bounce back |
| Pinch, ⌘+wheel | Zoom around the pointer | The playhead does not move |
| Click empty lane space or the ruler | The playhead moves to that time | Details do not open |
| Press and drag | Drags the playhead, auto-panning at the edges | No relayout or flicker while dragging |
| Click an episode, recording or screenshot | Positions: playhead to the item, stage shows it; merged blocks zoom in | Details do not open directly |
| Click a positioned item again, double-click, click the stage card, Return | Opens details; with details open, clicking a lane item switches the content directly | No detour through other content |
| Click an AI prompt dot | Unfolds in place | — |
| Playhead leaves the view | A "◀ time" tag appears at the edge; clicking it brings the view back | No automatic pull-back |
| "Back to now", jumps, arrow keys, playback | The view follows the playhead | — |

- **Following now**: for today, until the user positions something, the playhead advances with time and the view pans only when it nears the right edge; positioning or dragging stops following and shows "Back to now".
- **New data**: records arriving during dragging, scrolling or pinching merge about 0.3 s after the gesture ends; timeline layout is built in the background at most about every 0.5 s, and merging never moves the view.

<a id="menu-bar-panel"></a>

### 11.2 Menu bar panel

Clicking the menu bar icon toggles the panel; clicking outside, switching apps or pressing Esc closes it immediately. "Open main window" closes the panel after running; toggles keep it open. Implemented as an `NSStatusItem` with a transient `NSPopover` (`StatusItemController`). The icon is Candid's mark, whose dot shows capture state (coral recording, muted paused, hollow when something needs attention).

- Geometry rules (merging, folding, bucketing, heights) are implemented in `TimelineLaneLayout` with unit tests; values are defined only there and in `DesignSystem/`.
