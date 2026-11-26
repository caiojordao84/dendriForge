//! Pure-Rust headless workstation OS window manager (`docs/ui/DESIGN.md` §3.2, `F9`, `F10`, `F13`).
//!
//! Integer logical pixels, deterministic, zero `egui` dependency.
//! Owns two layout modes per numbered workspace (`1..=9`):
//! - **Floating** (KDE Plasma-inspired): z-ordered overlapping windows, pin-on-top elevation,
//!   4-edge & 4-corner resize, minimize/restore, maximize, reachable titlebar clamping,
//!   and screen-edge / corner quick-tiling snap zones.
//! - **Tiling (dwindle)** (Hyprland-inspired): automatic binary split tree along each leaf's
//!   longer axis, adjustable ratios & interactive gap dividers, per-window floating override,
//!   monocle maximize, directional focus & window swap, split-axis flip, and declarative
//!   window launch rules (`WindowRule`).
//!
//! Structural & geometric invariants `I1`–`I6` are checked transactionally on every mutation.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Default number of numbered workspaces created by [`Shell::try_new`].
pub const DEFAULT_WORKSPACES: u32 = 3;

/// Maximum number of numbered workspaces supported (`F13`).
pub const MAX_WORKSPACES: u32 = 9;

/// Default first-child share of a new split, in per-mille (`1..=999`).
const DEFAULT_RATIO: u16 = 500;

/// Minimum reachable title-bar strip inside workspace bounds, in logical pixels (WCAG 2.2 AA).
const VISIBLE_STRIP: i32 = 24;

/// Logical pixel rectangle. `w` and `h` are positive for every live window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    /// Right edge (`x + w`, saturating).
    #[must_use]
    pub const fn right(self) -> i32 {
        self.x.saturating_add(self.w)
    }

    /// Bottom edge (`y + h`, saturating).
    #[must_use]
    pub const fn bottom(self) -> i32 {
        self.y.saturating_add(self.h)
    }

    /// Returns `true` iff `(px, py)` lies within `[x, x + w) × [y, y + h)`.
    #[must_use]
    pub const fn contains_point(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
}

/// Identifier of one numbered workspace (`1..=9`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(u32);

impl WorkspaceId {
    pub const ONE: Self = Self(1);
    pub const TWO: Self = Self(2);
    pub const THREE: Self = Self(3);
    pub const FOUR: Self = Self(4);
    pub const FIVE: Self = Self(5);
    pub const SIX: Self = Self(6);
    pub const SEVEN: Self = Self(7);
    pub const EIGHT: Self = Self(8);
    pub const NINE: Self = Self(9);

    /// Constructs a validated workspace identifier in `1..=MAX_WORKSPACES`.
    pub const fn new(n: u32) -> Result<Self, WmError> {
        if n >= 1 && n <= MAX_WORKSPACES {
            Ok(Self(n))
        } else {
            Err(WmError::UnknownWorkspace(n))
        }
    }

    /// 1-based workspace number (`1..=9`).
    #[must_use]
    pub const fn number(self) -> u32 {
        self.0
    }
}

impl core::fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Resolves a 1-based workspace number (`1..=9`).
pub const fn workspace_id(n: u32) -> Result<WorkspaceId, WmError> {
    WorkspaceId::new(n)
}

/// Stable identifier of one window, allocated monotonically by [`Shell::open`] / [`Shell::open_with_rule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WindowId(u64);

impl WindowId {
    /// Raw numeric identifier.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl core::fmt::Display for WindowId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Layout mode for one workspace (`U1`). Floating is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Floating,
    Dwindle,
}

/// Cardinal direction for directional focus ([`Shell::focus_dir`]) and tiled swap ([`Shell::swap_dir`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Edge or corner handle for 8-way floating window resize (`F9`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeEdge {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

/// KDE Plasma-inspired screen-edge and corner quick-tiling snap zones (`F9`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapZone {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Maximize,
}

/// Declarative per-app window launch rule (`F13`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowRule {
    pub min_w: i32,
    pub min_h: i32,
    pub preferred_w: Option<i32>,
    pub preferred_h: Option<i32>,
    /// If `true`, window opens floating even inside a Tiling (dwindle) workspace (e.g. `Settings`).
    pub open_floating: bool,
    /// If `true`, initial floating rectangle is centered inside the workspace content area.
    pub centered: bool,
    /// If `true`, window starts pinned above non-pinned floating windows.
    pub pin_on_top: bool,
}

impl WindowRule {
    /// Standard tiled/floating rule with only minimum dimensions declared.
    #[must_use]
    pub const fn new(min_w: i32, min_h: i32) -> Self {
        Self {
            min_w,
            min_h,
            preferred_w: None,
            preferred_h: None,
            open_floating: false,
            centered: false,
            pin_on_top: false,
        }
    }

    /// Floating centered window rule (e.g. `Settings` app or modal tool window).
    #[must_use]
    pub const fn floating_centered(min_w: i32, min_h: i32, pref_w: i32, pref_h: i32) -> Self {
        Self {
            min_w,
            min_h,
            preferred_w: Some(pref_w),
            preferred_h: Some(pref_h),
            open_floating: true,
            centered: true,
            pin_on_top: false,
        }
    }
}

/// Display placement emitted by [`Shell::layout`] (back to front, pinned windows topmost).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub id: WindowId,
    pub rect: Rect,
    pub floating: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub pinned: bool,
    pub snap: Option<SnapZone>,
}

/// Interactive gap divider between two sibling subtrees in Dwindle mode (`F10`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitDivider {
    /// Representative leaf inside the first child subtree (passed to [`Shell::drag_divider`]).
    pub anchor_first: WindowId,
    /// Representative leaf inside the second child subtree.
    pub anchor_second: WindowId,
    /// `true` for a vertical divider bar separating left/right children; `false` for horizontal top/bottom.
    pub vertical: bool,
    /// Exact gap rectangle in logical pixels.
    pub rect: Rect,
    /// Parent split rectangle containing both children and the gap.
    pub parent_rect: Rect,
    /// Current split ratio in per-mille (`1..=999`).
    pub ratio: u16,
}

/// Failure returned by a window-manager operation.
/// [`WmError::Invariant`] indicates a structural audit failure; failed operations never mutate state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WmError {
    BadMin,
    BadBounds,
    BadRatio(u16),
    BadWorkspaceCount(u32),
    UnknownWindow(WindowId),
    UnknownWorkspace(u32),
    WorkspaceNotEmpty(WorkspaceId),
    BelowMin,
    DoesNotFit,
    NotFloating(WindowId),
    NotTiled(WindowId),
    NotInDwindle,
    NoNeighbor,
    EmptyFocus,
    Invariant(InvariantViolation),
}

impl core::fmt::Display for WmError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadMin => write!(f, "minimum size must be positive"),
            Self::BadBounds => write!(f, "bounds or gaps are invalid"),
            Self::BadRatio(ratio) => write!(f, "split ratio {ratio} is outside 1..=999"),
            Self::BadWorkspaceCount(n) => {
                write!(f, "workspace count {n} is outside 1..={MAX_WORKSPACES}")
            }
            Self::UnknownWindow(id) => write!(f, "unknown window {id}"),
            Self::UnknownWorkspace(id) => write!(f, "unknown workspace {id}"),
            Self::WorkspaceNotEmpty(id) => {
                write!(f, "workspace {id} still contains open windows")
            }
            Self::BelowMin => write!(f, "window would shrink below its minimum"),
            Self::DoesNotFit => write!(f, "layout does not fit the workspace"),
            Self::NotFloating(id) => write!(f, "window {id} is not floating"),
            Self::NotTiled(id) => write!(f, "window {id} is not in the dwindle tree"),
            Self::NotInDwindle => write!(f, "float toggle applies in Tiling (dwindle) mode"),
            Self::NoNeighbor => write!(f, "no window in that direction"),
            Self::EmptyFocus => write!(f, "workspace has no window to focus"),
            Self::Invariant(v) => write!(f, "invariant violated: {v}"),
        }
    }
}

impl std::error::Error for WmError {}

/// Broken structural or geometric invariant detected by [`Shell::audit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvariantViolation {
    Overlap {
        a: WindowId,
        b: WindowId,
    },
    Gap {
        expected: i32,
        found: i32,
    },
    Area {
        leaves: i64,
        gaps: i64,
        content: i64,
    },
    BelowMin {
        id: WindowId,
        w: i32,
        h: i32,
        min_w: i32,
        min_h: i32,
    },
    DoesNotFit,
    Orphan(WindowId),
    Missing(WindowId),
    Duplicate(WindowId),
    Focus,
    Monocle,
    ZOrder,
    BadRatio,
    MutatedOnError,
}

impl core::fmt::Display for InvariantViolation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Overlap { a, b } => write!(f, "tiled leaves {a} and {b} overlap"),
            Self::Gap { expected, found } => {
                write!(f, "sibling gap {found} does not equal {expected}")
            }
            Self::Area {
                leaves,
                gaps,
                content,
            } => write!(
                f,
                "leaf area {leaves} plus gaps {gaps} != content {content}"
            ),
            Self::BelowMin {
                id,
                w,
                h,
                min_w,
                min_h,
            } => write!(f, "window {id} is {w}x{h}, minimum {min_w}x{min_h}"),
            Self::DoesNotFit => write!(f, "partition does not fit"),
            Self::Orphan(id) => write!(f, "tree contains unexpected window {id}"),
            Self::Missing(id) => write!(f, "tiled window {id} is missing from the tree"),
            Self::Duplicate(id) => write!(f, "window {id} appears twice in the tree"),
            Self::Focus => write!(f, "focus is not a live window"),
            Self::Monocle => write!(f, "monocle target is not a live tiled window"),
            Self::ZOrder => write!(f, "z-order is not a permutation of the windows"),
            Self::BadRatio => write!(f, "stored split ratio is outside 1..=999"),
            Self::MutatedOnError => write!(f, "a refused operation changed the shell"),
        }
    }
}

impl std::error::Error for InvariantViolation {}

/// Result of feeding an arbitrary byte stream to [`fuzz_operation_sequence`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzReport {
    pub applied: u32,
    pub refused: u32,
    pub violations: Vec<InvariantViolation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Leaf(WindowId),
    Split {
        vertical: bool,
        ratio: u16,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    fn contains(&self, id: WindowId) -> bool {
        match self {
            Self::Leaf(found) => *found == id,
            Self::Split { first, second, .. } => first.contains(id) || second.contains(id),
        }
    }

    fn is_exact_leaf(&self, id: WindowId) -> bool {
        matches!(self, Self::Leaf(found) if *found == id)
    }

    fn first_leaf(&self) -> Option<WindowId> {
        match self {
            Self::Leaf(id) => Some(*id),
            Self::Split { first, .. } => first.first_leaf(),
        }
    }

    fn leaves(&self, out: &mut Vec<WindowId>) {
        match self {
            Self::Leaf(id) => out.push(*id),
            Self::Split { first, second, .. } => {
                first.leaves(out);
                second.leaves(out);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Window {
    min_w: i32,
    min_h: i32,
    floating: Rect,
    minimized: bool,
    maximized: bool,
    pinned: bool,
    snap: Option<SnapZone>,
    restore: Option<Rect>,
    tag: u64,
    floating_override: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Workspace {
    mode: Mode,
    windows: BTreeMap<WindowId, Window>,
    tree: Option<Node>,
    focus: Option<WindowId>,
    monocle: Option<WindowId>,
    /// Back to front z-order (excluding pin elevation applied at layout emission).
    z_order: Vec<WindowId>,
}

impl Workspace {
    fn empty() -> Self {
        Self {
            mode: Mode::Floating,
            windows: BTreeMap::new(),
            tree: None,
            focus: None,
            monocle: None,
            z_order: Vec::new(),
        }
    }
}

/// Pure, deterministic workstation OS window manager across numbered workspaces (`1..=9`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    bounds: Rect,
    gaps_in: i32,
    gaps_out: i32,
    visible_w: i32,
    visible_h: i32,
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    active: WorkspaceId,
    next_id: u64,
}

impl Shell {
    /// Builds a shell with [`DEFAULT_WORKSPACES`] (`1..=3`) workspaces in [`Mode::Floating`].
    pub fn try_new(bounds: Rect, gaps_in: i32, gaps_out: i32) -> Result<Self, WmError> {
        Self::try_new_with_workspaces(bounds, gaps_in, gaps_out, DEFAULT_WORKSPACES)
    }

    /// Builds a shell with `workspace_count` workspaces (`1..=MAX_WORKSPACES`) in [`Mode::Floating`].
    pub fn try_new_with_workspaces(
        bounds: Rect,
        gaps_in: i32,
        gaps_out: i32,
        workspace_count: u32,
    ) -> Result<Self, WmError> {
        if !(1..=MAX_WORKSPACES).contains(&workspace_count) {
            return Err(WmError::BadWorkspaceCount(workspace_count));
        }
        if gaps_in < 0 || gaps_out < 0 {
            return Err(WmError::BadBounds);
        }
        let _ = content_rect(bounds, gaps_out)?;
        let mut workspaces = BTreeMap::new();
        for n in 1..=workspace_count {
            let id = WorkspaceId::new(n)?;
            workspaces.insert(id, Workspace::empty());
        }
        Ok(Self {
            bounds,
            gaps_in,
            gaps_out,
            visible_w: VISIBLE_STRIP,
            visible_h: VISIBLE_STRIP,
            workspaces,
            active: WorkspaceId::ONE,
            next_id: 1,
        })
    }

    #[must_use]
    pub const fn bounds(&self) -> Rect {
        self.bounds
    }

    #[must_use]
    pub const fn gaps_in(&self) -> i32 {
        self.gaps_in
    }

    #[must_use]
    pub const fn gaps_out(&self) -> i32 {
        self.gaps_out
    }

    #[must_use]
    pub const fn active_workspace(&self) -> WorkspaceId {
        self.active
    }

    #[must_use]
    pub fn workspace_ids(&self) -> Vec<WorkspaceId> {
        self.workspaces.keys().copied().collect()
    }

    #[must_use]
    pub fn workspace_count(&self) -> u32 {
        u32::try_from(self.workspaces.len()).unwrap_or(MAX_WORKSPACES)
    }

    pub fn mode(&self) -> Result<Mode, WmError> {
        Ok(self.active_ws()?.mode)
    }

    pub fn workspace_mode(&self, ws: WorkspaceId) -> Result<Mode, WmError> {
        Ok(self.ws(ws)?.mode)
    }

    pub fn set_workspace_mode(&mut self, ws: WorkspaceId, mode: Mode) -> Result<(), WmError> {
        self.ws_mut(ws)?.mode = mode;
        Ok(())
    }

    pub fn focus(&self) -> Result<Option<WindowId>, WmError> {
        Ok(self.active_ws()?.focus)
    }

    pub fn monocle(&self) -> Result<Option<WindowId>, WmError> {
        Ok(self.active_ws()?.monocle)
    }

    pub fn tag(&self, id: WindowId) -> Result<u64, WmError> {
        let ws_id = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
        let ws = self.ws(ws_id)?;
        ws.windows
            .get(&id)
            .map(|window| window.tag)
            .ok_or(WmError::UnknownWindow(id))
    }

    #[must_use]
    pub fn window_workspace(&self, id: WindowId) -> Option<WorkspaceId> {
        self.locate(id)
    }

    pub fn is_pinned(&self, id: WindowId) -> Result<bool, WmError> {
        let ws_id = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
        self.ws(ws_id)?
            .windows
            .get(&id)
            .map(|w| w.pinned)
            .ok_or(WmError::UnknownWindow(id))
    }

    pub fn is_minimized(&self, id: WindowId) -> Result<bool, WmError> {
        let ws_id = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
        self.ws(ws_id)?
            .windows
            .get(&id)
            .map(|w| w.minimized)
            .ok_or(WmError::UnknownWindow(id))
    }

    pub fn is_floating(&self, id: WindowId) -> Result<bool, WmError> {
        self.display_floating(id)
    }

    /// Z-order of the active workspace, back to front.
    pub fn window_ids(&self) -> Result<Vec<WindowId>, WmError> {
        Ok(self.active_ws()?.z_order.clone())
    }

    /// Opens a window in the active workspace using default tiling/floating placement.
    pub fn open(&mut self, min_w: i32, min_h: i32, tag: u64) -> Result<WindowId, WmError> {
        self.open_with_rule(WindowRule::new(min_w, min_h), tag)
    }

    /// Opens a window in the active workspace honoring a declarative [`WindowRule`] (`F13`).
    pub fn open_with_rule(&mut self, rule: WindowRule, tag: u64) -> Result<WindowId, WmError> {
        if rule.min_w < 1 || rule.min_h < 1 {
            return Err(WmError::BadMin);
        }
        if let Some(pref_w) = rule.preferred_w
            && pref_w < rule.min_w
        {
            return Err(WmError::BadMin);
        }
        if let Some(pref_h) = rule.preferred_h
            && pref_h < rule.min_h
        {
            return Err(WmError::BadMin);
        }
        self.transact(|shell| shell.insert_window(rule, tag))
    }

    pub fn close(&mut self, id: WindowId) -> Result<(), WmError> {
        self.transact(|shell| shell.close_id(id))
    }

    /// Focuses `id` (restoring it if minimized, raising it to top of z-order, and clearing
    /// monocle if another tiled window receives focus).
    pub fn focus_window(&mut self, id: WindowId) -> Result<(), WmError> {
        self.transact(|shell| shell.focus_id(id))
    }

    pub fn focus_next(&mut self) -> Result<(), WmError> {
        self.transact(|shell| shell.cycle_focus(1))
    }

    pub fn focus_prev(&mut self) -> Result<(), WmError> {
        self.transact(|shell| shell.cycle_focus(-1))
    }

    pub fn focus_dir(&mut self, direction: Direction) -> Result<(), WmError> {
        self.transact(|shell| shell.focus_direction(direction))
    }

    pub fn move_by(&mut self, id: WindowId, dx: i32, dy: i32) -> Result<(), WmError> {
        self.transact(|shell| shell.move_floating(id, dx, dy))
    }

    pub fn resize(&mut self, id: WindowId, dw: i32, dh: i32) -> Result<(), WmError> {
        self.transact(|shell| shell.resize_window(id, dw, dh))
    }

    /// 8-way floating window resize from any edge or corner handle (`F9`).
    pub fn resize_edge(
        &mut self,
        id: WindowId,
        edge: ResizeEdge,
        dx: i32,
        dy: i32,
    ) -> Result<(), WmError> {
        self.transact(|shell| shell.resize_floating_edge(id, edge, dx, dy))
    }

    /// Computes the target logical rectangle for a Plasma-style quick-tile snap zone (`F9`).
    pub fn snap_preview(&self, zone: SnapZone) -> Result<Rect, WmError> {
        let content = content_rect(self.bounds, self.gaps_out)?;
        snap_rect(content, self.gaps_in, zone)
    }

    /// Detects whether a pointer at `(px, py)` lies inside an edge or corner snap hotzone (`F9`).
    #[must_use]
    pub fn detect_snap_zone(&self, px: i32, py: i32, threshold_px: i32) -> Option<SnapZone> {
        if threshold_px <= 0 || !self.bounds.contains_point(px, py) {
            return None;
        }
        let near_left = px.saturating_sub(self.bounds.x) < threshold_px;
        let near_right = self.bounds.right().saturating_sub(1).saturating_sub(px) < threshold_px;
        let near_top = py.saturating_sub(self.bounds.y) < threshold_px;
        let near_bottom = self.bounds.bottom().saturating_sub(1).saturating_sub(py) < threshold_px;

        match (near_left, near_right, near_top, near_bottom) {
            (true, false, true, false) => Some(SnapZone::TopLeft),
            (false, true, true, false) => Some(SnapZone::TopRight),
            (true, false, false, true) => Some(SnapZone::BottomLeft),
            (false, true, false, true) => Some(SnapZone::BottomRight),
            (true, false, false, false) => Some(SnapZone::LeftHalf),
            (false, true, false, false) => Some(SnapZone::RightHalf),
            (false, false, true, false) => Some(SnapZone::Maximize),
            (false, false, false, true) => Some(SnapZone::BottomHalf),
            _ => None,
        }
    }

    /// Snaps a floating window to a Plasma-style quick-tile zone (`F9`), enforcing `I3`.
    pub fn snap(&mut self, id: WindowId, zone: SnapZone) -> Result<(), WmError> {
        self.transact(|shell| shell.snap_floating(id, zone))
    }

    /// Restores a snapped or maximized floating window back to its pre-snap geometry (`F9`).
    pub fn unsnap(&mut self, id: WindowId) -> Result<(), WmError> {
        self.transact(|shell| shell.unsnap_floating(id))
    }

    /// Toggles KDE Plasma-style pin-on-top elevation for a floating window (`F9`).
    pub fn toggle_pin_on_top(&mut self, id: WindowId) -> Result<(), WmError> {
        self.transact(|shell| shell.toggle_pin_id(id))
    }

    /// Minimizes a floating window to the taskbar (`F9`).
    pub fn minimize(&mut self, id: WindowId) -> Result<(), WmError> {
        self.transact(|shell| shell.set_minimized_id(id, true))
    }

    /// Restores a minimized floating window and raises it to top of z-order (`F9`).
    pub fn restore(&mut self, id: WindowId) -> Result<(), WmError> {
        self.transact(|shell| shell.set_minimized_id(id, false))
    }

    /// Toggles minimized state for a floating window (`F9`).
    pub fn toggle_minimize(&mut self, id: WindowId) -> Result<(), WmError> {
        let currently = self.is_minimized(id)?;
        self.transact(|shell| shell.set_minimized_id(id, !currently))
    }

    /// Toggles a window between tiled and floating override inside a Dwindle workspace (`F10`).
    pub fn toggle_float(&mut self, id: WindowId) -> Result<(), WmError> {
        self.transact(|shell| shell.toggle_float_id(id))
    }

    /// Maximizes/restores a floating window (`F9`), or toggles Dwindle monocle for a tiled window (`F10`).
    pub fn maximize(&mut self, id: WindowId) -> Result<(), WmError> {
        self.transact(|shell| shell.maximize_id(id))
    }

    /// Swaps the focused tiled window with its nearest tiled neighbor in `direction` (`F10`).
    pub fn swap_dir(&mut self, direction: Direction) -> Result<(), WmError> {
        self.transact(|shell| shell.swap_tiled_dir(direction))
    }

    /// Flips the split axis (vertical ⇄ horizontal) of `id`'s parent split in Dwindle mode (`F10`).
    pub fn toggle_split_axis(&mut self, id: WindowId) -> Result<(), WmError> {
        self.transact(|shell| shell.flip_parent_axis(id))
    }

    pub fn move_to_workspace(&mut self, id: WindowId, dest: WorkspaceId) -> Result<(), WmError> {
        self.transact(|shell| shell.move_window(id, dest))
    }

    pub fn switch_workspace(&mut self, id: WorkspaceId) -> Result<(), WmError> {
        if !self.workspaces.contains_key(&id) {
            return Err(WmError::UnknownWorkspace(id.0));
        }
        self.active = id;
        Ok(())
    }

    /// Expands or shrinks the numbered workspace count (`1..=9`), refusing to remove any
    /// workspace that still contains windows (`F13`).
    pub fn set_workspace_count(&mut self, count: u32) -> Result<(), WmError> {
        if !(1..=MAX_WORKSPACES).contains(&count) {
            return Err(WmError::BadWorkspaceCount(count));
        }
        self.transact(|shell| {
            let current = shell.workspace_count();
            if count > current {
                for n in (current + 1)..=count {
                    let id = WorkspaceId::new(n)?;
                    shell.workspaces.insert(id, Workspace::empty());
                }
            } else if count < current {
                for n in (count + 1)..=current {
                    let id = WorkspaceId::new(n)?;
                    if let Some(ws) = shell.workspaces.get(&id)
                        && !ws.windows.is_empty()
                    {
                        return Err(WmError::WorkspaceNotEmpty(id));
                    }
                }
                for n in (count + 1)..=current {
                    let id = WorkspaceId::new(n)?;
                    shell.workspaces.remove(&id);
                }
                if shell.active.0 > count {
                    shell.active = WorkspaceId::new(count)?;
                }
            }
            Ok(())
        })
    }

    pub fn toggle_mode(&mut self) -> Result<(), WmError> {
        let ws = self.active_ws_mut()?;
        ws.mode = match ws.mode {
            Mode::Floating => Mode::Dwindle,
            Mode::Dwindle => Mode::Floating,
        };
        Ok(())
    }

    /// Sets the parent split ratio of `id` to `ratio` per-mille for the first child.
    pub fn set_ratio(&mut self, id: WindowId, ratio: u16) -> Result<(), WmError> {
        if !(1..1000).contains(&ratio) {
            return Err(WmError::BadRatio(ratio));
        }
        self.transact(|shell| shell.set_parent_ratio(id, ratio))
    }

    /// Returns interactive split gap dividers for the active workspace when in Dwindle mode (`F10`).
    pub fn dividers(&self) -> Result<Vec<SplitDivider>, WmError> {
        let ws = self.active_ws()?;
        if ws.mode != Mode::Dwindle || ws.monocle.is_some() {
            return Ok(Vec::new());
        }
        let Some(tree) = &ws.tree else {
            return Ok(Vec::new());
        };
        let content = content_rect(self.bounds, self.gaps_out)?;
        let mut out = Vec::new();
        collect_dividers(tree, content, self.gaps_in, &mut out).map_err(lift)?;
        Ok(out)
    }

    /// Adjusts the split divider identified by `anchor_first` by `delta_px` along its perpendicular axis (`F10`).
    pub fn drag_divider(&mut self, anchor_first: WindowId, delta_px: i32) -> Result<(), WmError> {
        self.transact(|shell| shell.drag_split_divider(anchor_first, delta_px))
    }

    pub fn set_bounds(&mut self, bounds: Rect) -> Result<(), WmError> {
        if bounds.w <= 0 || bounds.h <= 0 {
            return Err(WmError::BadBounds);
        }
        self.transact(|shell| {
            let _ = content_rect(bounds, shell.gaps_out)?;
            shell.bounds = bounds;
            Ok(())
        })
    }

    pub fn set_gaps(&mut self, gaps_in: i32, gaps_out: i32) -> Result<(), WmError> {
        if gaps_in < 0 || gaps_out < 0 {
            return Err(WmError::BadBounds);
        }
        self.transact(|shell| {
            let _ = content_rect(shell.bounds, gaps_out)?;
            shell.gaps_in = gaps_in;
            shell.gaps_out = gaps_out;
            Ok(())
        })
    }

    /// Display geometry of the active workspace, back to front.
    ///
    /// Ordering contract:
    /// 1. In Dwindle mode, tiled panes come first (back-to-front by z-order, with any
    ///    monocle window occupying `content_rect` on top of non-monocle tiled panes),
    ///    followed by non-pinned floating overrides, followed by pinned floating overrides.
    /// 2. In Floating mode, non-pinned windows come first in z-order, followed by pinned
    ///    windows in z-order.
    pub fn layout(&self) -> Result<Vec<Placed>, WmError> {
        let ws = self.active_ws()?;
        let content = content_rect(self.bounds, self.gaps_out)?;
        let tiled = match &ws.tree {
            Some(tree) => measure(tree, content, self.gaps_in).map_err(lift)?.leaves,
            None => Vec::new(),
        };
        let mut by_id = BTreeMap::new();
        for (id, rect) in tiled {
            by_id.insert(id, rect);
        }

        let mut layer_tiled = Vec::new();
        let mut layer_monocle = Vec::new();
        let mut layer_float = Vec::new();
        let mut layer_pinned = Vec::new();

        for id in &ws.z_order {
            let window = ws.windows.get(id).ok_or(WmError::UnknownWindow(*id))?;
            let floating = ws.mode == Mode::Floating || window.floating_override;
            if floating {
                let placed = Placed {
                    id: *id,
                    rect: window.floating,
                    floating: true,
                    minimized: window.minimized,
                    maximized: window.maximized,
                    pinned: window.pinned,
                    snap: window.snap,
                };
                if window.pinned {
                    layer_pinned.push(placed);
                } else {
                    layer_float.push(placed);
                }
            } else {
                let is_monocle = ws.monocle == Some(*id);
                let rect = if is_monocle {
                    content
                } else {
                    *by_id
                        .get(id)
                        .ok_or(WmError::Invariant(InvariantViolation::Missing(*id)))?
                };
                let placed = Placed {
                    id: *id,
                    rect,
                    floating: false,
                    minimized: false,
                    maximized: is_monocle,
                    pinned: false,
                    snap: None,
                };
                if is_monocle {
                    layer_monocle.push(placed);
                } else {
                    layer_tiled.push(placed);
                }
            }
        }

        let mut out = Vec::with_capacity(ws.z_order.len());
        out.extend(layer_tiled);
        out.extend(layer_monocle);
        out.extend(layer_float);
        out.extend(layer_pinned);
        Ok(out)
    }

    /// Tiled partition of the active workspace, depth-first. Empty when nothing is tiled.
    pub fn partition(&self) -> Result<Vec<(WindowId, Rect)>, WmError> {
        let ws = self.active_ws()?;
        let content = content_rect(self.bounds, self.gaps_out)?;
        match &ws.tree {
            None => Ok(Vec::new()),
            Some(tree) => Ok(measure(tree, content, self.gaps_in).map_err(lift)?.leaves),
        }
    }

    /// Verifies invariant **I1**: tiled leaves never overlap and respect `gaps_in`.
    pub fn check_i1(&self) -> Result<(), InvariantViolation> {
        for ws in self.workspaces.values() {
            let Some(tree) = &ws.tree else {
                continue;
            };
            let content = content_rect(self.bounds, self.gaps_out)
                .map_err(|_| InvariantViolation::DoesNotFit)?;
            check_gaps(tree, content, self.gaps_in)?;
            let leaves = measure(tree, content, self.gaps_in)?.leaves;
            for (index, (id_a, rect_a)) in leaves.iter().enumerate() {
                for (id_b, rect_b) in leaves.iter().skip(index + 1) {
                    if overlaps(*rect_a, *rect_b) {
                        return Err(InvariantViolation::Overlap { a: *id_a, b: *id_b });
                    }
                }
            }
        }
        Ok(())
    }

    /// Verifies invariant **I2**: tiled leaves plus inner gaps cover the exact content area.
    pub fn check_i2(&self) -> Result<(), InvariantViolation> {
        for ws in self.workspaces.values() {
            let Some(tree) = &ws.tree else {
                continue;
            };
            let content = content_rect(self.bounds, self.gaps_out)
                .map_err(|_| InvariantViolation::DoesNotFit)?;
            let measured = measure(tree, content, self.gaps_in)?;
            let leaf_area = measured.leaf_area;
            let gap_area = measured.gap_area;
            let content_area = i64::from(content.w)
                .checked_mul(i64::from(content.h))
                .ok_or(InvariantViolation::DoesNotFit)?;
            let covered = leaf_area
                .checked_add(gap_area)
                .ok_or(InvariantViolation::DoesNotFit)?;
            if covered != content_area {
                return Err(InvariantViolation::Area {
                    leaves: leaf_area,
                    gaps: gap_area,
                    content: content_area,
                });
            }
        }
        Ok(())
    }

    /// Verifies invariant **I3**: no floating or tiled window shrinks below its declared minimum.
    pub fn check_i3(&self) -> Result<(), InvariantViolation> {
        for ws in self.workspaces.values() {
            for (id, window) in &ws.windows {
                if window.floating.w < window.min_w || window.floating.h < window.min_h {
                    return Err(InvariantViolation::BelowMin {
                        id: *id,
                        w: window.floating.w,
                        h: window.floating.h,
                        min_w: window.min_w,
                        min_h: window.min_h,
                    });
                }
            }
            let Some(tree) = &ws.tree else {
                continue;
            };
            let content = content_rect(self.bounds, self.gaps_out)
                .map_err(|_| InvariantViolation::DoesNotFit)?;
            for (id, rect) in measure(tree, content, self.gaps_in)?.leaves {
                let Some(window) = ws.windows.get(&id) else {
                    return Err(InvariantViolation::Orphan(id));
                };
                if rect.w < window.min_w || rect.h < window.min_h {
                    return Err(InvariantViolation::BelowMin {
                        id,
                        w: rect.w,
                        h: rect.h,
                        min_w: window.min_w,
                        min_h: window.min_h,
                    });
                }
            }
        }
        Ok(())
    }

    /// Runs the complete structural and `I1–I3` audit across every workspace.
    pub fn audit(&self) -> Result<(), InvariantViolation> {
        self.check_structure()?;
        self.check_i1()?;
        self.check_i2()?;
        self.check_i3()?;
        Ok(())
    }

    fn transact<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, WmError>,
    ) -> Result<T, WmError> {
        let mut trial = self.clone();
        let value = f(&mut trial)?;
        match trial.audit() {
            Ok(()) => {
                *self = trial;
                Ok(value)
            }
            Err(InvariantViolation::BelowMin { .. }) => Err(WmError::BelowMin),
            Err(InvariantViolation::DoesNotFit) => Err(WmError::DoesNotFit),
            Err(violation) => Err(WmError::Invariant(violation)),
        }
    }

    fn insert_window(&mut self, rule: WindowRule, tag: u64) -> Result<WindowId, WmError> {
        let content = content_rect(self.bounds, self.gaps_out)?;
        let floating = initial_float(content, rule)?;
        let gaps_in = self.gaps_in;
        let active = self.active;

        let anchor = if rule.open_floating {
            None
        } else {
            let ws = self.ws(active)?;
            match &ws.tree {
                None => None,
                Some(tree) => {
                    let anchor =
                        tiled_anchor(ws).ok_or(WmError::Invariant(InvariantViolation::Focus))?;
                    let leaves = measure(tree, content, gaps_in).map_err(lift)?.leaves;
                    let rect = leaves
                        .into_iter()
                        .find(|(id, _)| *id == anchor)
                        .map(|(_, rect)| rect)
                        .ok_or(WmError::Invariant(InvariantViolation::Missing(anchor)))?;
                    Some((anchor, rect))
                }
            }
        };

        let id = WindowId(self.next_id);
        self.next_id = self.next_id.checked_add(1).ok_or(WmError::DoesNotFit)?;
        let ws = self.ws_mut(active)?;
        if !rule.open_floating {
            match anchor {
                None => ws.tree = Some(Node::Leaf(id)),
                Some((anchor_id, anchor_rect)) => {
                    let tree = ws
                        .tree
                        .take()
                        .ok_or(WmError::Invariant(InvariantViolation::Focus))?;
                    ws.tree = Some(split_at(tree, anchor_id, id, anchor_rect)?);
                }
            }
            // Opening a new tiled window exits monocle so the new pane is visible.
            ws.monocle = None;
        }
        ws.windows.insert(
            id,
            Window {
                min_w: rule.min_w,
                min_h: rule.min_h,
                floating,
                minimized: false,
                maximized: false,
                pinned: rule.pin_on_top,
                snap: None,
                restore: None,
                tag,
                floating_override: rule.open_floating,
            },
        );
        ws.z_order.push(id);
        ws.focus = Some(id);
        Ok(id)
    }

    fn close_id(&mut self, id: WindowId) -> Result<(), WmError> {
        let src = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
        let ws = self.ws_mut(src)?;
        detach_from_tree(ws, id)?;
        ws.windows.remove(&id);
        ws.z_order.retain(|window| *window != id);
        if ws.monocle == Some(id) {
            ws.monocle = None;
        }
        if ws.focus == Some(id) {
            ws.focus = topmost_visible(ws).or_else(|| ws.z_order.last().copied());
        }
        Ok(())
    }

    fn focus_id(&mut self, id: WindowId) -> Result<(), WmError> {
        let ws_id = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
        let ws = self.ws_mut(ws_id)?;
        let is_override = ws
            .windows
            .get(&id)
            .map(|w| w.floating_override)
            .ok_or(WmError::UnknownWindow(id))?;
        if let Some(window) = ws.windows.get_mut(&id) {
            window.minimized = false;
        }
        ws.z_order.retain(|w| *w != id);
        ws.z_order.push(id);
        ws.focus = Some(id);
        if ws.mode == Mode::Dwindle && !is_override && ws.monocle != Some(id) {
            ws.monocle = None;
        }
        Ok(())
    }

    fn cycle_focus(&mut self, delta: i32) -> Result<(), WmError> {
        let ws = self.active_ws_mut()?;
        let len = ws.z_order.len();
        if len == 0 {
            return Err(WmError::EmptyFocus);
        }
        let current = ws
            .focus
            .and_then(|id| ws.z_order.iter().position(|window| *window == id));
        let next = match current {
            Some(index) => wrap_index(index, len, delta),
            None => len - 1,
        };
        let next_id = ws.z_order.get(next).copied();
        ws.focus = next_id;
        if let Some(id) = next_id {
            if let Some(window) = ws.windows.get_mut(&id) {
                window.minimized = false;
            }
            if ws.monocle.is_some() && ws.monocle != Some(id) {
                let is_tiled = ws.windows.get(&id).is_some_and(|w| !w.floating_override);
                if is_tiled {
                    ws.monocle = None;
                }
            }
        }
        Ok(())
    }

    fn focus_direction(&mut self, direction: Direction) -> Result<(), WmError> {
        let placed = self.layout()?;
        let focus = self.focus()?.ok_or(WmError::EmptyFocus)?;
        let origin = placed
            .iter()
            .find(|placed| placed.id == focus)
            .ok_or(WmError::UnknownWindow(focus))?;
        let (ox, oy) = center(origin.rect);
        let mut best: Option<(i64, i64, WindowId)> = None;
        for candidate in &placed {
            if candidate.id == focus || candidate.minimized {
                continue;
            }
            let (cx, cy) = center(candidate.rect);
            let (primary, secondary) = directional_distance(ox, oy, cx, cy, direction);
            if primary <= 0 {
                continue;
            }
            let key = (primary, secondary, candidate.id);
            if best.is_none_or(|current| key < current) {
                best = Some(key);
            }
        }
        let Some((_, _, id)) = best else {
            return Err(WmError::NoNeighbor);
        };
        self.focus_id(id)
    }

    fn move_floating(&mut self, id: WindowId, dx: i32, dy: i32) -> Result<(), WmError> {
        if !self.display_floating(id)? {
            return Err(WmError::NotFloating(id));
        }
        let bounds = self.bounds;
        let visible_w = self.visible_w;
        let visible_h = self.visible_h;
        let window = self.window_mut(id)?;
        let mut rect = window.floating;
        rect.x = rect.x.checked_add(dx).ok_or(WmError::DoesNotFit)?;
        rect.y = rect.y.checked_add(dy).ok_or(WmError::DoesNotFit)?;
        window.floating = clamp_origin(rect, bounds, visible_w, visible_h);
        window.maximized = false;
        window.snap = None;
        window.restore = None;
        Ok(())
    }

    fn resize_window(&mut self, id: WindowId, dw: i32, dh: i32) -> Result<(), WmError> {
        if self.display_floating(id)? {
            let window = self.window_mut(id)?;
            let w = window
                .floating
                .w
                .checked_add(dw)
                .ok_or(WmError::DoesNotFit)?;
            let h = window
                .floating
                .h
                .checked_add(dh)
                .ok_or(WmError::DoesNotFit)?;
            if w < window.min_w || h < window.min_h || w < 1 || h < 1 {
                return Err(WmError::BelowMin);
            }
            window.floating.w = w;
            window.floating.h = h;
            window.maximized = false;
            window.snap = None;
            window.restore = None;
            Ok(())
        } else {
            let content = content_rect(self.bounds, self.gaps_out)?;
            let gaps_in = self.gaps_in;
            let ws_id = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
            let ws = self.ws_mut(ws_id)?;
            let Some(tree) = ws.tree.as_mut() else {
                return Err(WmError::NotTiled(id));
            };
            let changed = adjust_leaf(tree, id, content, gaps_in, dw, dh)?;
            if !changed {
                return Err(WmError::NotTiled(id));
            }
            Ok(())
        }
    }

    fn resize_floating_edge(
        &mut self,
        id: WindowId,
        edge: ResizeEdge,
        dx: i32,
        dy: i32,
    ) -> Result<(), WmError> {
        if !self.display_floating(id)? {
            return Err(WmError::NotFloating(id));
        }
        let bounds = self.bounds;
        let visible_w = self.visible_w;
        let visible_h = self.visible_h;
        let window = self.window_mut(id)?;
        let mut rect = window.floating;

        let (move_left, grow_right) = match edge {
            ResizeEdge::West | ResizeEdge::NorthWest | ResizeEdge::SouthWest => (true, false),
            ResizeEdge::East | ResizeEdge::NorthEast | ResizeEdge::SouthEast => (false, true),
            ResizeEdge::North | ResizeEdge::South => (false, false),
        };
        let (move_top, grow_bottom) = match edge {
            ResizeEdge::North | ResizeEdge::NorthWest | ResizeEdge::NorthEast => (true, false),
            ResizeEdge::South | ResizeEdge::SouthWest | ResizeEdge::SouthEast => (false, true),
            ResizeEdge::West | ResizeEdge::East => (false, false),
        };

        if move_left {
            let next_w = rect.w.checked_sub(dx).ok_or(WmError::DoesNotFit)?;
            let next_x = rect.x.checked_add(dx).ok_or(WmError::DoesNotFit)?;
            rect.x = next_x;
            rect.w = next_w;
        } else if grow_right {
            rect.w = rect.w.checked_add(dx).ok_or(WmError::DoesNotFit)?;
        }

        if move_top {
            let next_h = rect.h.checked_sub(dy).ok_or(WmError::DoesNotFit)?;
            let next_y = rect.y.checked_add(dy).ok_or(WmError::DoesNotFit)?;
            rect.y = next_y;
            rect.h = next_h;
        } else if grow_bottom {
            rect.h = rect.h.checked_add(dy).ok_or(WmError::DoesNotFit)?;
        }

        if rect.w < window.min_w || rect.h < window.min_h || rect.w < 1 || rect.h < 1 {
            return Err(WmError::BelowMin);
        }

        window.floating = clamp_origin(rect, bounds, visible_w, visible_h);
        window.maximized = false;
        window.snap = None;
        window.restore = None;
        Ok(())
    }

    fn snap_floating(&mut self, id: WindowId, zone: SnapZone) -> Result<(), WmError> {
        if !self.display_floating(id)? {
            return Err(WmError::NotFloating(id));
        }
        let target = self.snap_preview(zone)?;
        let window = self.window_mut(id)?;
        if target.w < window.min_w || target.h < window.min_h {
            return Err(WmError::BelowMin);
        }
        if window.restore.is_none() {
            window.restore = Some(window.floating);
        }
        window.floating = target;
        window.minimized = false;
        window.snap = Some(zone);
        window.maximized = zone == SnapZone::Maximize;
        Ok(())
    }

    fn unsnap_floating(&mut self, id: WindowId) -> Result<(), WmError> {
        if !self.display_floating(id)? {
            return Err(WmError::NotFloating(id));
        }
        let window = self.window_mut(id)?;
        if let Some(prev) = window.restore.take() {
            window.floating = prev;
        }
        window.snap = None;
        window.maximized = false;
        Ok(())
    }

    fn toggle_pin_id(&mut self, id: WindowId) -> Result<(), WmError> {
        if !self.display_floating(id)? {
            return Err(WmError::NotFloating(id));
        }
        let window = self.window_mut(id)?;
        window.pinned = !window.pinned;
        Ok(())
    }

    fn set_minimized_id(&mut self, id: WindowId, minimized: bool) -> Result<(), WmError> {
        if !self.display_floating(id)? {
            return Err(WmError::NotFloating(id));
        }
        let ws_id = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
        let ws = self.ws_mut(ws_id)?;
        let window = ws.windows.get_mut(&id).ok_or(WmError::UnknownWindow(id))?;
        window.minimized = minimized;
        if minimized {
            if ws.focus == Some(id) {
                ws.focus = topmost_visible(ws).or(Some(id));
            }
        } else {
            ws.z_order.retain(|w| *w != id);
            ws.z_order.push(id);
            ws.focus = Some(id);
        }
        Ok(())
    }

    fn toggle_float_id(&mut self, id: WindowId) -> Result<(), WmError> {
        let content = content_rect(self.bounds, self.gaps_out)?;
        let gaps_in = self.gaps_in;
        let ws_id = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
        let ws = self.ws_mut(ws_id)?;
        if ws.mode != Mode::Dwindle {
            return Err(WmError::NotInDwindle);
        }
        let override_flag = ws
            .windows
            .get(&id)
            .map(|window| window.floating_override)
            .ok_or(WmError::UnknownWindow(id))?;
        if override_flag {
            // Re-tile into the dwindle tree (even if the tree was empty).
            match ws.tree.take() {
                None => {
                    ws.tree = Some(Node::Leaf(id));
                }
                Some(tree) => {
                    let anchor = tiled_anchor_from(&tree, ws.focus)
                        .ok_or(WmError::Invariant(InvariantViolation::Focus))?;
                    let rect = measure(&tree, content, gaps_in)
                        .map_err(lift)?
                        .leaves
                        .into_iter()
                        .find(|(leaf, _)| *leaf == anchor)
                        .map(|(_, rect)| rect)
                        .ok_or(WmError::Invariant(InvariantViolation::Missing(anchor)))?;
                    ws.tree = Some(split_at(tree, anchor, id, rect)?);
                }
            }
            let window = ws.windows.get_mut(&id).ok_or(WmError::UnknownWindow(id))?;
            window.floating_override = false;
            window.minimized = false;
            ws.monocle = None;
            Ok(())
        } else {
            detach_from_tree(ws, id)?;
            if ws.monocle == Some(id) {
                ws.monocle = None;
            }
            let window = ws.windows.get_mut(&id).ok_or(WmError::UnknownWindow(id))?;
            window.floating_override = true;
            Ok(())
        }
    }

    fn maximize_id(&mut self, id: WindowId) -> Result<(), WmError> {
        if self.display_floating(id)? {
            let content = content_rect(self.bounds, self.gaps_out)?;
            let window = self.window_mut(id)?;
            if window.maximized {
                if let Some(previous) = window.restore.take() {
                    window.floating = previous;
                }
                window.maximized = false;
                window.snap = None;
            } else {
                if window.restore.is_none() {
                    window.restore = Some(window.floating);
                }
                window.floating = content;
                window.maximized = true;
                window.snap = Some(SnapZone::Maximize);
                window.minimized = false;
            }
            Ok(())
        } else {
            let ws_id = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
            let ws = self.ws_mut(ws_id)?;
            if ws.monocle == Some(id) {
                ws.monocle = None;
            } else {
                ws.monocle = Some(id);
                ws.focus = Some(id);
            }
            Ok(())
        }
    }

    fn swap_tiled_dir(&mut self, direction: Direction) -> Result<(), WmError> {
        let ws = self.active_ws()?;
        if ws.mode != Mode::Dwindle {
            return Err(WmError::NotInDwindle);
        }
        let focus = ws.focus.ok_or(WmError::EmptyFocus)?;
        let is_override = ws
            .windows
            .get(&focus)
            .map(|w| w.floating_override)
            .ok_or(WmError::UnknownWindow(focus))?;
        if is_override {
            return Err(WmError::NotTiled(focus));
        }
        let leaves = self.partition()?;
        let origin = leaves
            .iter()
            .find(|(id, _)| *id == focus)
            .map(|(_, r)| *r)
            .ok_or(WmError::NotTiled(focus))?;
        let (ox, oy) = center(origin);
        let mut best: Option<(i64, i64, WindowId)> = None;
        for (candidate_id, candidate_rect) in &leaves {
            if *candidate_id == focus {
                continue;
            }
            let (cx, cy) = center(*candidate_rect);
            let (primary, secondary) = directional_distance(ox, oy, cx, cy, direction);
            if primary <= 0 {
                continue;
            }
            let key = (primary, secondary, *candidate_id);
            if best.is_none_or(|current| key < current) {
                best = Some(key);
            }
        }
        let Some((_, _, target)) = best else {
            return Err(WmError::NoNeighbor);
        };
        let ws = self.active_ws_mut()?;
        let Some(tree) = ws.tree.as_mut() else {
            return Err(WmError::NotTiled(focus));
        };
        swap_leaves(tree, focus, target);
        Ok(())
    }

    fn flip_parent_axis(&mut self, id: WindowId) -> Result<(), WmError> {
        let ws_id = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
        let ws = self.ws_mut(ws_id)?;
        if ws.mode != Mode::Dwindle {
            return Err(WmError::NotInDwindle);
        }
        let Some(tree) = ws.tree.as_mut() else {
            return Err(WmError::NotTiled(id));
        };
        if !flip_parent_split(tree, id) {
            return Err(WmError::NotTiled(id));
        }
        Ok(())
    }

    fn drag_split_divider(&mut self, anchor_first: WindowId, delta_px: i32) -> Result<(), WmError> {
        let content = content_rect(self.bounds, self.gaps_out)?;
        let gaps_in = self.gaps_in;
        let ws = self.active_ws_mut()?;
        if ws.mode != Mode::Dwindle {
            return Err(WmError::NotInDwindle);
        }
        let Some(tree) = ws.tree.as_mut() else {
            return Err(WmError::NotTiled(anchor_first));
        };
        let changed = drag_divider_node(tree, anchor_first, content, gaps_in, delta_px)?;
        if !changed {
            return Err(WmError::NotTiled(anchor_first));
        }
        Ok(())
    }

    fn move_window(&mut self, id: WindowId, dest: WorkspaceId) -> Result<(), WmError> {
        if !self.workspaces.contains_key(&dest) {
            return Err(WmError::UnknownWorkspace(dest.0));
        }
        let src = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
        if src == dest {
            return Ok(());
        }
        let content = content_rect(self.bounds, self.gaps_out)?;
        let gaps_in = self.gaps_in;
        let window = {
            let ws = self.ws_mut(src)?;
            detach_from_tree(ws, id)?;
            let window = ws.windows.remove(&id).ok_or(WmError::UnknownWindow(id))?;
            ws.z_order.retain(|window_id| *window_id != id);
            if ws.monocle == Some(id) {
                ws.monocle = None;
            }
            if ws.focus == Some(id) {
                ws.focus = topmost_visible(ws).or_else(|| ws.z_order.last().copied());
            }
            window
        };
        self.insert_existing(dest, id, window, content, gaps_in)?;
        Ok(())
    }

    fn insert_existing(
        &mut self,
        dest: WorkspaceId,
        id: WindowId,
        window: Window,
        content: Rect,
        gaps_in: i32,
    ) -> Result<(), WmError> {
        let anchor = if window.floating_override {
            None
        } else {
            let ws = self.ws(dest)?;
            match &ws.tree {
                None => None,
                Some(tree) => {
                    let anchor =
                        tiled_anchor(ws).ok_or(WmError::Invariant(InvariantViolation::Focus))?;
                    let rect = measure(tree, content, gaps_in)
                        .map_err(lift)?
                        .leaves
                        .into_iter()
                        .find(|(leaf, _)| *leaf == anchor)
                        .map(|(_, rect)| rect)
                        .ok_or(WmError::Invariant(InvariantViolation::Missing(anchor)))?;
                    Some((anchor, rect))
                }
            }
        };
        let ws = self.ws_mut(dest)?;
        if !window.floating_override {
            match anchor {
                None => ws.tree = Some(Node::Leaf(id)),
                Some((anchor_id, anchor_rect)) => {
                    let tree = ws
                        .tree
                        .take()
                        .ok_or(WmError::Invariant(InvariantViolation::Focus))?;
                    ws.tree = Some(split_at(tree, anchor_id, id, anchor_rect)?);
                }
            }
            ws.monocle = None;
        }
        ws.windows.insert(id, window);
        ws.z_order.push(id);
        ws.focus = Some(id);
        Ok(())
    }

    fn set_parent_ratio(&mut self, id: WindowId, ratio: u16) -> Result<(), WmError> {
        let ws_id = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
        let ws = self.ws_mut(ws_id)?;
        let Some(tree) = ws.tree.as_mut() else {
            return Err(WmError::NotTiled(id));
        };
        if !assign_parent_ratio(tree, id, ratio) {
            return Err(WmError::NotTiled(id));
        }
        Ok(())
    }

    fn check_structure(&self) -> Result<(), InvariantViolation> {
        for ws in self.workspaces.values() {
            let mut leaves = Vec::new();
            if let Some(tree) = &ws.tree {
                tree.leaves(&mut leaves);
            }
            let mut seen = BTreeSet::new();
            for id in &leaves {
                if !seen.insert(*id) {
                    return Err(InvariantViolation::Duplicate(*id));
                }
            }
            let mut tiled = 0usize;
            for (id, window) in &ws.windows {
                if window.floating_override {
                    if seen.contains(id) {
                        return Err(InvariantViolation::Orphan(*id));
                    }
                } else {
                    tiled += 1;
                    if !seen.contains(id) {
                        return Err(InvariantViolation::Missing(*id));
                    }
                }
            }
            if tiled != seen.len() {
                return Err(match leaves.first().copied() {
                    Some(id) => InvariantViolation::Orphan(id),
                    None => InvariantViolation::Focus,
                });
            }
            let mut z_seen = BTreeSet::new();
            for id in &ws.z_order {
                if !z_seen.insert(*id) || !ws.windows.contains_key(id) {
                    return Err(InvariantViolation::ZOrder);
                }
            }
            if z_seen.len() != ws.windows.len() {
                return Err(InvariantViolation::ZOrder);
            }
            if let Some(id) = ws.focus
                && !ws.windows.contains_key(&id)
            {
                return Err(InvariantViolation::Focus);
            }
            if let Some(mono_id) = ws.monocle {
                let valid = ws
                    .windows
                    .get(&mono_id)
                    .is_some_and(|w| !w.floating_override)
                    && seen.contains(&mono_id);
                if !valid {
                    return Err(InvariantViolation::Monocle);
                }
            }
        }
        Ok(())
    }

    fn active_ws(&self) -> Result<&Workspace, WmError> {
        self.ws(self.active)
    }

    fn active_ws_mut(&mut self) -> Result<&mut Workspace, WmError> {
        self.ws_mut(self.active)
    }

    fn ws(&self, id: WorkspaceId) -> Result<&Workspace, WmError> {
        self.workspaces
            .get(&id)
            .ok_or(WmError::UnknownWorkspace(id.0))
    }

    fn ws_mut(&mut self, id: WorkspaceId) -> Result<&mut Workspace, WmError> {
        self.workspaces
            .get_mut(&id)
            .ok_or(WmError::UnknownWorkspace(id.0))
    }

    fn locate(&self, id: WindowId) -> Option<WorkspaceId> {
        self.workspaces
            .iter()
            .find(|(_, ws)| ws.windows.contains_key(&id))
            .map(|(ws_id, _)| *ws_id)
    }

    fn window_mut(&mut self, id: WindowId) -> Result<&mut Window, WmError> {
        let ws_id = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
        self.ws_mut(ws_id)?
            .windows
            .get_mut(&id)
            .ok_or(WmError::UnknownWindow(id))
    }

    fn display_floating(&self, id: WindowId) -> Result<bool, WmError> {
        let ws_id = self.locate(id).ok_or(WmError::UnknownWindow(id))?;
        let ws = self.ws(ws_id)?;
        let window = ws.windows.get(&id).ok_or(WmError::UnknownWindow(id))?;
        Ok(ws.mode == Mode::Floating || window.floating_override)
    }

    fn all_ids(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        for ws in self.workspaces.values() {
            out.extend(ws.z_order.iter().copied());
        }
        out
    }
}

fn topmost_visible(ws: &Workspace) -> Option<WindowId> {
    ws.z_order.iter().rev().copied().find(|id| {
        ws.windows
            .get(id)
            .is_some_and(|w| !w.minimized || (ws.mode == Mode::Dwindle && !w.floating_override))
    })
}

fn tiled_anchor(ws: &Workspace) -> Option<WindowId> {
    ws.tree
        .as_ref()
        .and_then(|tree| tiled_anchor_from(tree, ws.focus))
}

fn tiled_anchor_from(tree: &Node, focus: Option<WindowId>) -> Option<WindowId> {
    if let Some(id) = focus
        && tree.contains(id)
    {
        return Some(id);
    }
    tree.first_leaf()
}

fn detach_from_tree(ws: &mut Workspace, id: WindowId) -> Result<(), WmError> {
    let in_tree = ws.tree.as_ref().is_some_and(|tree| tree.contains(id));
    if !in_tree {
        return Ok(());
    }
    let tree = ws
        .tree
        .take()
        .ok_or(WmError::Invariant(InvariantViolation::Focus))?;
    ws.tree = remove_leaf(tree, id)?;
    Ok(())
}

fn lift(violation: InvariantViolation) -> WmError {
    match violation {
        InvariantViolation::BelowMin { .. } => WmError::BelowMin,
        InvariantViolation::DoesNotFit => WmError::DoesNotFit,
        other @ (InvariantViolation::Overlap { .. }
        | InvariantViolation::Gap { .. }
        | InvariantViolation::Area { .. }
        | InvariantViolation::Orphan(_)
        | InvariantViolation::Missing(_)
        | InvariantViolation::Duplicate(_)
        | InvariantViolation::Focus
        | InvariantViolation::Monocle
        | InvariantViolation::ZOrder
        | InvariantViolation::BadRatio
        | InvariantViolation::MutatedOnError) => WmError::Invariant(other),
    }
}

fn content_rect(bounds: Rect, gaps_out: i32) -> Result<Rect, WmError> {
    if bounds.w <= 0 || bounds.h <= 0 || gaps_out < 0 {
        return Err(WmError::BadBounds);
    }
    let inset = gaps_out.checked_mul(2).ok_or(WmError::DoesNotFit)?;
    if bounds.w <= inset || bounds.h <= inset {
        return Err(WmError::DoesNotFit);
    }
    let x = bounds.x.checked_add(gaps_out).ok_or(WmError::DoesNotFit)?;
    let y = bounds.y.checked_add(gaps_out).ok_or(WmError::DoesNotFit)?;
    Ok(Rect {
        x,
        y,
        w: bounds.w - inset,
        h: bounds.h - inset,
    })
}

fn snap_rect(content: Rect, gap: i32, zone: SnapZone) -> Result<Rect, WmError> {
    match zone {
        SnapZone::Maximize => Ok(content),
        SnapZone::LeftHalf => Ok(split_rect(content, true, DEFAULT_RATIO, gap)
            .map_err(lift)?
            .0),
        SnapZone::RightHalf => Ok(split_rect(content, true, DEFAULT_RATIO, gap)
            .map_err(lift)?
            .1),
        SnapZone::TopHalf => Ok(split_rect(content, false, DEFAULT_RATIO, gap)
            .map_err(lift)?
            .0),
        SnapZone::BottomHalf => Ok(split_rect(content, false, DEFAULT_RATIO, gap)
            .map_err(lift)?
            .1),
        SnapZone::TopLeft => {
            let (left, _) = split_rect(content, true, DEFAULT_RATIO, gap).map_err(lift)?;
            Ok(split_rect(left, false, DEFAULT_RATIO, gap).map_err(lift)?.0)
        }
        SnapZone::BottomLeft => {
            let (left, _) = split_rect(content, true, DEFAULT_RATIO, gap).map_err(lift)?;
            Ok(split_rect(left, false, DEFAULT_RATIO, gap).map_err(lift)?.1)
        }
        SnapZone::TopRight => {
            let (_, right) = split_rect(content, true, DEFAULT_RATIO, gap).map_err(lift)?;
            Ok(split_rect(right, false, DEFAULT_RATIO, gap)
                .map_err(lift)?
                .0)
        }
        SnapZone::BottomRight => {
            let (_, right) = split_rect(content, true, DEFAULT_RATIO, gap).map_err(lift)?;
            Ok(split_rect(right, false, DEFAULT_RATIO, gap)
                .map_err(lift)?
                .1)
        }
    }
}

fn initial_float(content: Rect, rule: WindowRule) -> Result<Rect, WmError> {
    if content.w < rule.min_w || content.h < rule.min_h {
        return Err(WmError::BelowMin);
    }
    let target_w = rule.preferred_w.unwrap_or_else(|| rule.min_w.max(200));
    let target_h = rule.preferred_h.unwrap_or_else(|| rule.min_h.max(120));
    let w = target_w.max(rule.min_w).min(content.w);
    let h = target_h.max(rule.min_h).min(content.h);
    if w < rule.min_w || h < rule.min_h {
        return Err(WmError::BelowMin);
    }
    let (x, y) = if rule.centered {
        (
            content.x.saturating_add((content.w - w) / 2),
            content.y.saturating_add((content.h - h) / 2),
        )
    } else {
        (content.x, content.y)
    };
    Ok(Rect { x, y, w, h })
}

fn split_at(
    node: Node,
    anchor: WindowId,
    new_id: WindowId,
    anchor_rect: Rect,
) -> Result<Node, WmError> {
    match node {
        Node::Leaf(id) if id == anchor => {
            let vertical = anchor_rect.w >= anchor_rect.h;
            Ok(Node::Split {
                vertical,
                ratio: DEFAULT_RATIO,
                first: Box::new(Node::Leaf(id)),
                second: Box::new(Node::Leaf(new_id)),
            })
        }
        Node::Leaf(id) => Err(WmError::Invariant(InvariantViolation::Missing(id))),
        Node::Split {
            vertical,
            ratio,
            first,
            second,
        } => {
            let in_first = first.contains(anchor);
            let in_second = second.contains(anchor);
            if in_first {
                let first = split_at(*first, anchor, new_id, anchor_rect)?;
                Ok(Node::Split {
                    vertical,
                    ratio,
                    first: Box::new(first),
                    second,
                })
            } else if in_second {
                let second = split_at(*second, anchor, new_id, anchor_rect)?;
                Ok(Node::Split {
                    vertical,
                    ratio,
                    first,
                    second: Box::new(second),
                })
            } else {
                Err(WmError::Invariant(InvariantViolation::Missing(anchor)))
            }
        }
    }
}

fn remove_leaf(node: Node, id: WindowId) -> Result<Option<Node>, WmError> {
    match node {
        Node::Leaf(found) if found == id => Ok(None),
        Node::Leaf(_) => Err(WmError::Invariant(InvariantViolation::Missing(id))),
        Node::Split {
            vertical,
            ratio,
            first,
            second,
        } => {
            let in_first = first.contains(id);
            let in_second = second.contains(id);
            if in_first {
                match remove_leaf(*first, id)? {
                    None => Ok(Some(*second)),
                    Some(next) => Ok(Some(Node::Split {
                        vertical,
                        ratio,
                        first: Box::new(next),
                        second,
                    })),
                }
            } else if in_second {
                match remove_leaf(*second, id)? {
                    None => Ok(Some(*first)),
                    Some(next) => Ok(Some(Node::Split {
                        vertical,
                        ratio,
                        first,
                        second: Box::new(next),
                    })),
                }
            } else {
                Err(WmError::Invariant(InvariantViolation::Missing(id)))
            }
        }
    }
}

fn swap_leaves(node: &mut Node, a: WindowId, b: WindowId) {
    match node {
        Node::Leaf(id) => {
            if *id == a {
                *id = b;
            } else if *id == b {
                *id = a;
            }
        }
        Node::Split { first, second, .. } => {
            swap_leaves(first, a, b);
            swap_leaves(second, a, b);
        }
    }
}

fn flip_parent_split(node: &mut Node, id: WindowId) -> bool {
    let Node::Split {
        vertical,
        ratio: _,
        first,
        second,
    } = node
    else {
        return false;
    };
    if first.is_exact_leaf(id) || second.is_exact_leaf(id) {
        *vertical = !*vertical;
        return true;
    }
    if first.contains(id) {
        flip_parent_split(first, id)
    } else if second.contains(id) {
        flip_parent_split(second, id)
    } else {
        false
    }
}

fn assign_parent_ratio(node: &mut Node, id: WindowId, ratio: u16) -> bool {
    let Node::Split {
        vertical: _,
        ratio: current,
        first,
        second,
    } = node
    else {
        return false;
    };
    if first.is_exact_leaf(id) || second.is_exact_leaf(id) {
        *current = ratio;
        return true;
    }
    if first.contains(id) {
        assign_parent_ratio(first, id, ratio)
    } else if second.contains(id) {
        assign_parent_ratio(second, id, ratio)
    } else {
        false
    }
}

fn collect_dividers(
    node: &Node,
    rect: Rect,
    gap: i32,
    out: &mut Vec<SplitDivider>,
) -> Result<(), InvariantViolation> {
    let Node::Split {
        vertical,
        ratio,
        first,
        second,
    } = node
    else {
        return Ok(());
    };
    let (r1, r2) = split_rect(rect, *vertical, *ratio, gap)?;
    if let (Some(a1), Some(a2)) = (first.first_leaf(), second.first_leaf()) {
        let divider_rect = if *vertical {
            Rect {
                x: r1.right(),
                y: rect.y,
                w: gap,
                h: rect.h,
            }
        } else {
            Rect {
                x: rect.x,
                y: r1.bottom(),
                w: rect.w,
                h: gap,
            }
        };
        out.push(SplitDivider {
            anchor_first: a1,
            anchor_second: a2,
            vertical: *vertical,
            rect: divider_rect,
            parent_rect: rect,
            ratio: *ratio,
        });
    }
    collect_dividers(first, r1, gap, out)?;
    collect_dividers(second, r2, gap, out)?;
    Ok(())
}

fn drag_divider_node(
    node: &mut Node,
    anchor_first: WindowId,
    rect: Rect,
    gap: i32,
    delta_px: i32,
) -> Result<bool, WmError> {
    let Node::Split {
        vertical,
        ratio,
        first,
        second,
    } = node
    else {
        return Ok(false);
    };
    let (r1, r2) = split_rect(rect, *vertical, *ratio, gap).map_err(lift)?;
    if first.first_leaf() == Some(anchor_first) {
        let new_ratio = ratio_from_px(r1, delta_px, rect, *vertical, gap).map_err(lift)?;
        *ratio = new_ratio;
        return Ok(true);
    }
    if first.contains(anchor_first) {
        drag_divider_node(first, anchor_first, r1, gap, delta_px)
    } else if second.contains(anchor_first) {
        drag_divider_node(second, anchor_first, r2, gap, delta_px)
    } else {
        Ok(false)
    }
}

fn adjust_leaf(
    node: &mut Node,
    id: WindowId,
    rect: Rect,
    gap: i32,
    dw: i32,
    dh: i32,
) -> Result<bool, WmError> {
    let (vertical, r1, r2, first_exact, second_exact, in_first, in_second) = match node {
        Node::Leaf(_) => return Ok(false),
        Node::Split {
            vertical,
            ratio,
            first,
            second,
        } => {
            let (r1, r2) = split_rect(rect, *vertical, *ratio, gap).map_err(lift)?;
            (
                *vertical,
                r1,
                r2,
                first.is_exact_leaf(id),
                second.is_exact_leaf(id),
                first.contains(id),
                second.contains(id),
            )
        }
    };
    if first_exact || second_exact {
        let delta = if vertical { dw } else { dh };
        let signed = if first_exact {
            delta
        } else {
            delta.saturating_neg()
        };
        let new_ratio = ratio_from_px(r1, signed, rect, vertical, gap).map_err(lift)?;
        if let Node::Split { ratio, .. } = node {
            *ratio = new_ratio;
        }
        return Ok(true);
    }
    match node {
        Node::Split { first, second, .. } => {
            if in_first {
                adjust_leaf(first, id, r1, gap, dw, dh)
            } else if in_second {
                adjust_leaf(second, id, r2, gap, dw, dh)
            } else {
                Ok(false)
            }
        }
        Node::Leaf(_) => Ok(false),
    }
}

fn ratio_from_px(
    first: Rect,
    delta: i32,
    parent: Rect,
    vertical: bool,
    gap: i32,
) -> Result<u16, InvariantViolation> {
    let usable = if vertical {
        parent.w.checked_sub(gap)
    } else {
        parent.h.checked_sub(gap)
    }
    .ok_or(InvariantViolation::DoesNotFit)?;
    let current = if vertical { first.w } else { first.h };
    let new_first = current
        .checked_add(delta)
        .ok_or(InvariantViolation::DoesNotFit)?;
    if usable < 2 || new_first < 1 || new_first >= usable {
        return Err(InvariantViolation::DoesNotFit);
    }
    let ratio = i64::from(new_first) * 1000 / i64::from(usable);
    let ratio = u16::try_from(ratio).map_err(|_| InvariantViolation::DoesNotFit)?;
    if !(1..1000).contains(&ratio) {
        return Err(InvariantViolation::DoesNotFit);
    }
    Ok(ratio)
}

fn split_rect(
    rect: Rect,
    vertical: bool,
    ratio: u16,
    gap: i32,
) -> Result<(Rect, Rect), InvariantViolation> {
    if !(1..1000).contains(&ratio) {
        return Err(InvariantViolation::BadRatio);
    }
    if gap < 0 || rect.w <= 0 || rect.h <= 0 {
        return Err(InvariantViolation::DoesNotFit);
    }
    if vertical {
        let usable = rect
            .w
            .checked_sub(gap)
            .ok_or(InvariantViolation::DoesNotFit)?;
        if usable < 2 {
            return Err(InvariantViolation::DoesNotFit);
        }
        let first_w = mul_ratio(usable, ratio)?;
        let second_w = usable
            .checked_sub(first_w)
            .ok_or(InvariantViolation::DoesNotFit)?;
        if first_w < 1 || second_w < 1 {
            return Err(InvariantViolation::DoesNotFit);
        }
        let x2 = rect
            .x
            .checked_add(first_w)
            .ok_or(InvariantViolation::DoesNotFit)?
            .checked_add(gap)
            .ok_or(InvariantViolation::DoesNotFit)?;
        Ok((
            Rect {
                x: rect.x,
                y: rect.y,
                w: first_w,
                h: rect.h,
            },
            Rect {
                x: x2,
                y: rect.y,
                w: second_w,
                h: rect.h,
            },
        ))
    } else {
        let usable = rect
            .h
            .checked_sub(gap)
            .ok_or(InvariantViolation::DoesNotFit)?;
        if usable < 2 {
            return Err(InvariantViolation::DoesNotFit);
        }
        let first_h = mul_ratio(usable, ratio)?;
        let second_h = usable
            .checked_sub(first_h)
            .ok_or(InvariantViolation::DoesNotFit)?;
        if first_h < 1 || second_h < 1 {
            return Err(InvariantViolation::DoesNotFit);
        }
        let y2 = rect
            .y
            .checked_add(first_h)
            .ok_or(InvariantViolation::DoesNotFit)?
            .checked_add(gap)
            .ok_or(InvariantViolation::DoesNotFit)?;
        Ok((
            Rect {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: first_h,
            },
            Rect {
                x: rect.x,
                y: y2,
                w: rect.w,
                h: second_h,
            },
        ))
    }
}

fn mul_ratio(usable: i32, ratio: u16) -> Result<i32, InvariantViolation> {
    let value = i64::from(usable) * i64::from(ratio) / 1000;
    i32::try_from(value).map_err(|_| InvariantViolation::DoesNotFit)
}

struct Measured {
    leaf_area: i64,
    gap_area: i64,
    leaves: Vec<(WindowId, Rect)>,
}

fn measure(node: &Node, rect: Rect, gap: i32) -> Result<Measured, InvariantViolation> {
    match node {
        Node::Leaf(id) => {
            let leaf_area = i64::from(rect.w)
                .checked_mul(i64::from(rect.h))
                .ok_or(InvariantViolation::DoesNotFit)?;
            Ok(Measured {
                leaf_area,
                gap_area: 0,
                leaves: vec![(*id, rect)],
            })
        }
        Node::Split {
            vertical,
            ratio,
            first,
            second,
        } => {
            let (r1, r2) = split_rect(rect, *vertical, *ratio, gap)?;
            let gap_here = if *vertical {
                i64::from(gap).checked_mul(i64::from(rect.h))
            } else {
                i64::from(gap).checked_mul(i64::from(rect.w))
            }
            .ok_or(InvariantViolation::DoesNotFit)?;
            let left = measure(first, r1, gap)?;
            let right = measure(second, r2, gap)?;
            let mut leaves = left.leaves;
            leaves.extend(right.leaves);
            let leaf_area = left
                .leaf_area
                .checked_add(right.leaf_area)
                .ok_or(InvariantViolation::DoesNotFit)?;
            let gap_area = left
                .gap_area
                .checked_add(right.gap_area)
                .and_then(|sum| sum.checked_add(gap_here))
                .ok_or(InvariantViolation::DoesNotFit)?;
            Ok(Measured {
                leaf_area,
                gap_area,
                leaves,
            })
        }
    }
}

fn check_gaps(node: &Node, rect: Rect, gap: i32) -> Result<(), InvariantViolation> {
    let Node::Split {
        vertical,
        ratio,
        first,
        second,
    } = node
    else {
        return Ok(());
    };
    let (r1, r2) = split_rect(rect, *vertical, *ratio, gap)?;
    let found = if *vertical {
        r2.x.checked_sub(r1.x.saturating_add(r1.w))
    } else {
        r2.y.checked_sub(r1.y.saturating_add(r1.h))
    }
    .ok_or(InvariantViolation::DoesNotFit)?;
    if found != gap {
        return Err(InvariantViolation::Gap {
            expected: gap,
            found,
        });
    }
    check_gaps(first, r1, gap)?;
    check_gaps(second, r2, gap)?;
    Ok(())
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.x.saturating_add(b.w)
        && b.x < a.x.saturating_add(a.w)
        && a.y < b.y.saturating_add(b.h)
        && b.y < a.y.saturating_add(a.h)
}

fn center(rect: Rect) -> (i64, i64) {
    (
        i64::from(rect.x) + i64::from(rect.w) / 2,
        i64::from(rect.y) + i64::from(rect.h) / 2,
    )
}

fn directional_distance(ox: i64, oy: i64, cx: i64, cy: i64, direction: Direction) -> (i64, i64) {
    match direction {
        Direction::Left => (ox - cx, (oy - cy).abs()),
        Direction::Right => (cx - ox, (oy - cy).abs()),
        Direction::Up => (oy - cy, (ox - cx).abs()),
        Direction::Down => (cy - oy, (ox - cx).abs()),
    }
}

fn wrap_index(index: usize, len: usize, delta: i32) -> usize {
    if len == 0 {
        return 0;
    }
    let len_i = i64::try_from(len).unwrap_or(1);
    let index_i = i64::try_from(index).unwrap_or(0);
    let shifted = index_i + i64::from(delta);
    let modulo = shifted.rem_euclid(len_i);
    usize::try_from(modulo).unwrap_or(0)
}

fn clamp_i32(value: i32, lo: i32, hi: i32) -> i32 {
    let hi = if hi < lo { lo } else { hi };
    if value < lo {
        lo
    } else if value > hi {
        hi
    } else {
        value
    }
}

fn clamp_origin(rect: Rect, bounds: Rect, vis_w: i32, vis_h: i32) -> Rect {
    let vis_w = vis_w.max(1).min(bounds.w.max(1));
    let vis_h = vis_h.max(1).min(bounds.h.max(1));
    let max_x = bounds.x.saturating_add((bounds.w - vis_w).max(0));
    let max_y = bounds.y.saturating_add((bounds.h - vis_h).max(0));
    Rect {
        x: clamp_i32(rect.x, bounds.x, max_x),
        y: clamp_i32(rect.y, bounds.y, max_y),
        w: rect.w,
        h: rect.h,
    }
}

// ---------------------------------------------------------------------------
// Deterministic Fuzz Driver (`cargo-fuzz` + `proptest`)
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    data: &'a [u8],
    index: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, index: 0 }
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.data.get(self.index).copied()?;
        self.index += 1;
        Some(byte)
    }
}

enum Step {
    Done,
    Act(Result<(), WmError>),
}

fn nth_id(ids: &[WindowId], byte: u8) -> Option<WindowId> {
    let len = ids.len();
    if len == 0 {
        return None;
    }
    ids.get(usize::from(byte) % len).copied()
}

fn direction_from(byte: u8) -> Direction {
    match byte % 4 {
        0 => Direction::Left,
        1 => Direction::Right,
        2 => Direction::Up,
        _ => Direction::Down,
    }
}

fn edge_from(byte: u8) -> ResizeEdge {
    match byte % 8 {
        0 => ResizeEdge::North,
        1 => ResizeEdge::South,
        2 => ResizeEdge::East,
        3 => ResizeEdge::West,
        4 => ResizeEdge::NorthEast,
        5 => ResizeEdge::NorthWest,
        6 => ResizeEdge::SouthEast,
        _ => ResizeEdge::SouthWest,
    }
}

fn snap_from(byte: u8) -> SnapZone {
    match byte % 9 {
        0 => SnapZone::LeftHalf,
        1 => SnapZone::RightHalf,
        2 => SnapZone::TopHalf,
        3 => SnapZone::BottomHalf,
        4 => SnapZone::TopLeft,
        5 => SnapZone::TopRight,
        6 => SnapZone::BottomLeft,
        7 => SnapZone::BottomRight,
        _ => SnapZone::Maximize,
    }
}

fn fuzz_open(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(byte) = bytes.next() else {
        return Step::Done;
    };
    let min = 8 + i32::from(byte % 24);
    if byte & 0x80 != 0 {
        let rule = WindowRule {
            min_w: min,
            min_h: min,
            preferred_w: Some(min + 40),
            preferred_h: Some(min + 30),
            open_floating: true,
            centered: byte & 0x40 != 0,
            pin_on_top: byte & 0x20 != 0,
        };
        Step::Act(shell.open_with_rule(rule, u64::from(byte)).map(|_| ()))
    } else {
        Step::Act(shell.open(min, min, u64::from(byte)).map(|_| ()))
    }
}

fn fuzz_close(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(byte) = bytes.next() else {
        return Step::Done;
    };
    let ids = shell.all_ids();
    let Some(id) = nth_id(&ids, byte) else {
        return Step::Act(Err(WmError::EmptyFocus));
    };
    Step::Act(shell.close(id))
}

fn fuzz_focus_dir(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(byte) = bytes.next() else {
        return Step::Done;
    };
    Step::Act(shell.focus_dir(direction_from(byte)))
}

fn fuzz_swap_dir(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(byte) = bytes.next() else {
        return Step::Done;
    };
    Step::Act(shell.swap_dir(direction_from(byte)))
}

fn fuzz_move(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(which) = bytes.next() else {
        return Step::Done;
    };
    let Some(dx) = bytes.next() else {
        return Step::Done;
    };
    let Some(dy) = bytes.next() else {
        return Step::Done;
    };
    let ids = shell.all_ids();
    let Some(id) = nth_id(&ids, which) else {
        return Step::Act(Err(WmError::EmptyFocus));
    };
    Step::Act(shell.move_by(
        id,
        i8::from_le_bytes([dx]).into(),
        i8::from_le_bytes([dy]).into(),
    ))
}

fn fuzz_resize(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(which) = bytes.next() else {
        return Step::Done;
    };
    let Some(dw) = bytes.next() else {
        return Step::Done;
    };
    let Some(dh) = bytes.next() else {
        return Step::Done;
    };
    let ids = shell.all_ids();
    let Some(id) = nth_id(&ids, which) else {
        return Step::Act(Err(WmError::EmptyFocus));
    };
    Step::Act(shell.resize(
        id,
        i8::from_le_bytes([dw]).into(),
        i8::from_le_bytes([dh]).into(),
    ))
}

fn fuzz_resize_edge(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(which) = bytes.next() else {
        return Step::Done;
    };
    let Some(edge_b) = bytes.next() else {
        return Step::Done;
    };
    let Some(dx) = bytes.next() else {
        return Step::Done;
    };
    let Some(dy) = bytes.next() else {
        return Step::Done;
    };
    let ids = shell.all_ids();
    let Some(id) = nth_id(&ids, which) else {
        return Step::Act(Err(WmError::EmptyFocus));
    };
    Step::Act(shell.resize_edge(
        id,
        edge_from(edge_b),
        i8::from_le_bytes([dx]).into(),
        i8::from_le_bytes([dy]).into(),
    ))
}

fn fuzz_snap(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(which) = bytes.next() else {
        return Step::Done;
    };
    let Some(zone_b) = bytes.next() else {
        return Step::Done;
    };
    let ids = shell.all_ids();
    let Some(id) = nth_id(&ids, which) else {
        return Step::Act(Err(WmError::EmptyFocus));
    };
    Step::Act(shell.snap(id, snap_from(zone_b)))
}

fn fuzz_on_window(
    shell: &mut Shell,
    bytes: &mut Cursor<'_>,
    op: impl FnOnce(&mut Shell, WindowId) -> Result<(), WmError>,
) -> Step {
    let Some(byte) = bytes.next() else {
        return Step::Done;
    };
    let ids = shell.all_ids();
    let Some(id) = nth_id(&ids, byte) else {
        return Step::Act(Err(WmError::EmptyFocus));
    };
    Step::Act(op(shell, id))
}

fn fuzz_move_workspace(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(which) = bytes.next() else {
        return Step::Done;
    };
    let Some(dest_byte) = bytes.next() else {
        return Step::Done;
    };
    let ids = shell.all_ids();
    let Some(id) = nth_id(&ids, which) else {
        return Step::Act(Err(WmError::EmptyFocus));
    };
    let count = shell.workspace_count().max(1);
    let dest = match workspace_id((u32::from(dest_byte) % count) + 1) {
        Ok(id) => id,
        Err(error) => return Step::Act(Err(error)),
    };
    Step::Act(shell.move_to_workspace(id, dest))
}

fn fuzz_switch(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(byte) = bytes.next() else {
        return Step::Done;
    };
    let count = shell.workspace_count().max(1);
    match workspace_id((u32::from(byte) % count) + 1) {
        Ok(id) => Step::Act(shell.switch_workspace(id)),
        Err(error) => Step::Act(Err(error)),
    }
}

fn fuzz_ratio(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(which) = bytes.next() else {
        return Step::Done;
    };
    let Some(ratio_byte) = bytes.next() else {
        return Step::Done;
    };
    let ids = shell.all_ids();
    let Some(id) = nth_id(&ids, which) else {
        return Step::Act(Err(WmError::EmptyFocus));
    };
    let ratio = u16::from(ratio_byte % 249) + 1;
    let ratio = ratio.saturating_mul(4).clamp(1, 999);
    Step::Act(shell.set_ratio(id, ratio))
}

fn fuzz_bounds(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(width_byte) = bytes.next() else {
        return Step::Done;
    };
    let Some(height_byte) = bytes.next() else {
        return Step::Done;
    };
    let widths = [400, 640, 800, 1024, 1280, 1600, 1920, 2560];
    let heights = [300, 480, 600, 720, 800, 900, 1080, 1440];
    let width = match widths.get(usize::from(width_byte) % widths.len()).copied() {
        Some(value) => value,
        None => return Step::Act(Err(WmError::BadBounds)),
    };
    let height = match heights
        .get(usize::from(height_byte) % heights.len())
        .copied()
    {
        Some(value) => value,
        None => return Step::Act(Err(WmError::BadBounds)),
    };
    Step::Act(shell.set_bounds(Rect {
        x: 0,
        y: 0,
        w: width,
        h: height,
    }))
}

fn fuzz_workspace_count(shell: &mut Shell, bytes: &mut Cursor<'_>) -> Step {
    let Some(byte) = bytes.next() else {
        return Step::Done;
    };
    let target = u32::from(byte % 9) + 1;
    Step::Act(shell.set_workspace_count(target))
}

/// Applies a deterministic operation sequence from raw bytes, auditing `I1–I3` and
/// structural integrity after every applied operation.
pub fn fuzz_operation_sequence(data: &[u8]) -> (Shell, FuzzReport) {
    let mut report = FuzzReport {
        applied: 0,
        refused: 0,
        violations: Vec::new(),
    };
    let Ok(mut shell) = Shell::try_new(
        Rect {
            x: 0,
            y: 0,
            w: 800,
            h: 600,
        },
        4,
        4,
    ) else {
        report.violations.push(InvariantViolation::DoesNotFit);
        let fallback = Shell {
            bounds: Rect {
                x: 0,
                y: 0,
                w: 800,
                h: 600,
            },
            gaps_in: 4,
            gaps_out: 4,
            visible_w: VISIBLE_STRIP,
            visible_h: VISIBLE_STRIP,
            workspaces: BTreeMap::new(),
            active: WorkspaceId::ONE,
            next_id: 1,
        };
        return (fallback, report);
    };
    let mut bytes = Cursor::new(data);
    while let Some(op) = bytes.next() {
        let step = match op % 22 {
            0 => fuzz_open(&mut shell, &mut bytes),
            1 => fuzz_close(&mut shell, &mut bytes),
            2 => Step::Act(shell.focus_next()),
            3 => Step::Act(shell.focus_prev()),
            4 => fuzz_focus_dir(&mut shell, &mut bytes),
            5 => fuzz_move(&mut shell, &mut bytes),
            6 => fuzz_resize(&mut shell, &mut bytes),
            7 => fuzz_resize_edge(&mut shell, &mut bytes),
            8 => fuzz_snap(&mut shell, &mut bytes),
            9 => fuzz_on_window(&mut shell, &mut bytes, Shell::unsnap),
            10 => fuzz_on_window(&mut shell, &mut bytes, Shell::toggle_pin_on_top),
            11 => fuzz_on_window(&mut shell, &mut bytes, Shell::toggle_minimize),
            12 => fuzz_on_window(&mut shell, &mut bytes, Shell::toggle_float),
            13 => fuzz_on_window(&mut shell, &mut bytes, Shell::maximize),
            14 => fuzz_swap_dir(&mut shell, &mut bytes),
            15 => fuzz_on_window(&mut shell, &mut bytes, Shell::toggle_split_axis),
            16 => fuzz_move_workspace(&mut shell, &mut bytes),
            17 => fuzz_switch(&mut shell, &mut bytes),
            18 => Step::Act(shell.toggle_mode()),
            19 => fuzz_ratio(&mut shell, &mut bytes),
            20 => fuzz_bounds(&mut shell, &mut bytes),
            21 => fuzz_workspace_count(&mut shell, &mut bytes),
            _ => Step::Act(Err(WmError::EmptyFocus)),
        };
        let action = match step {
            Step::Done => break,
            Step::Act(action) => action,
        };
        match action {
            Ok(()) => {
                report.applied += 1;
                if let Err(violation) = shell.audit() {
                    report.violations.push(violation);
                    break;
                }
            }
            Err(WmError::Invariant(violation)) => {
                report.violations.push(violation);
                break;
            }
            Err(_) => {
                report.refused += 1;
            }
        }
    }
    (shell, report)
}

#[cfg(test)]
fn sibling_subtree(node: &Node, id: WindowId) -> Option<Node> {
    let Node::Split { first, second, .. } = node else {
        return None;
    };
    if first.is_exact_leaf(id) {
        return Some(second.as_ref().clone());
    }
    if second.is_exact_leaf(id) {
        return Some(first.as_ref().clone());
    }
    if first.contains(id) {
        return sibling_subtree(first, id);
    }
    if second.contains(id) {
        sibling_subtree(second, id)
    } else {
        None
    }
}

#[cfg(test)]
fn contains_subtree(hay: &Node, needle: &Node) -> bool {
    if hay == needle {
        return true;
    }
    match hay {
        Node::Leaf(_) => false,
        Node::Split { first, second, .. } => {
            contains_subtree(first, needle) || contains_subtree(second, needle)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::collection::vec as byte_vec;
    use proptest::prelude::*;
    use proptest::test_runner::Config as ProptestConfig;

    fn canvas() -> Shell {
        Shell::try_new(
            Rect {
                x: 0,
                y: 0,
                w: 108,
                h: 108,
            },
            10,
            4,
        )
        .expect("canvas")
    }

    fn desktop() -> Shell {
        Shell::try_new(
            Rect {
                x: 0,
                y: 0,
                w: 1280,
                h: 800,
            },
            8,
            8,
        )
        .expect("desktop")
    }

    fn clean(data: &[u8]) -> Shell {
        let (shell, report) = fuzz_operation_sequence(data);
        assert!(report.violations.is_empty(), "{:?}", report.violations);
        assert!(shell.audit().is_ok());
        shell
    }

    #[test]
    fn i2_three_pane_area_and_first_leaf_stable() {
        let mut shell = canvas();
        let first = shell.open(10, 10, 1).unwrap();
        let second = shell.open(10, 10, 2).unwrap();
        let third = shell.open(10, 10, 3).unwrap();
        shell.check_i1().unwrap();
        shell.check_i2().unwrap();
        shell.check_i3().unwrap();
        let parts = shell.partition().unwrap();
        assert_eq!(
            parts,
            vec![
                (
                    first,
                    Rect {
                        x: 4,
                        y: 4,
                        w: 45,
                        h: 100
                    }
                ),
                (
                    second,
                    Rect {
                        x: 59,
                        y: 4,
                        w: 45,
                        h: 45
                    }
                ),
                (
                    third,
                    Rect {
                        x: 59,
                        y: 59,
                        w: 45,
                        h: 45
                    }
                ),
            ]
        );
        shell.close(third).unwrap();
        assert_eq!(
            shell.partition().unwrap(),
            vec![
                (
                    first,
                    Rect {
                        x: 4,
                        y: 4,
                        w: 45,
                        h: 100
                    }
                ),
                (
                    second,
                    Rect {
                        x: 59,
                        y: 4,
                        w: 45,
                        h: 100
                    }
                ),
            ]
        );
    }

    #[test]
    fn i6_close_promotes_sibling_unchanged() {
        let mut shell = canvas();
        let first = shell.open(10, 10, 1).unwrap();
        shell.open(10, 10, 2).unwrap();
        shell.open(10, 10, 3).unwrap();
        shell.open(10, 10, 4).unwrap();
        let before = shell.active_ws().unwrap().tree.clone().unwrap();
        let sibling = sibling_subtree(&before, first).unwrap();
        shell.close(first).unwrap();
        let after = shell.active_ws().unwrap().tree.clone().unwrap();
        assert_eq!(after, sibling);
        shell.check_i1().unwrap();
        shell.check_i2().unwrap();
    }

    #[test]
    fn i4_toggle_mode_preserves_windows_focus_and_tags() {
        let mut shell = canvas();
        shell.open(20, 20, 7).unwrap();
        shell.open(20, 20, 9).unwrap();
        let before = shell.clone();
        shell.toggle_mode().unwrap();
        assert!(same_except_active_mode(&before, &shell));
        shell.toggle_mode().unwrap();
        assert_eq!(shell, before);
    }

    #[test]
    fn clamp_keeps_title_inside_bounds() {
        let mut shell = Shell::try_new(
            Rect {
                x: 0,
                y: 0,
                w: 200,
                h: 100,
            },
            0,
            0,
        )
        .unwrap();
        let id = shell.open(24, 24, 1).unwrap();
        shell.move_by(id, 10_000, 10_000).unwrap();
        let placed = shell.layout().unwrap();
        let rect = placed.iter().find(|p| p.id == id).unwrap().rect;
        assert_eq!((rect.x, rect.y), (176, 76));
        shell.move_by(id, -20_000, -20_000).unwrap();
        let placed = shell.layout().unwrap();
        let rect = placed.iter().find(|p| p.id == id).unwrap().rect;
        assert_eq!((rect.x, rect.y), (0, 0));
    }

    #[test]
    fn i3_refused_open_does_not_change_state() {
        let shell = canvas();
        let mut refused = shell.clone();
        assert!(refused.open(10_000, 10_000, 1).is_err());
        assert_eq!(refused, shell);
    }

    #[test]
    fn f9_plasma_snap_zones_pin_on_top_and_8_way_resize() {
        let mut shell = desktop();
        let a = shell.open(200, 150, 1).unwrap();
        let b = shell.open(200, 150, 2).unwrap();

        // Detect hotzones along edges & corners
        assert_eq!(shell.detect_snap_zone(4, 4, 16), Some(SnapZone::TopLeft));
        assert_eq!(
            shell.detect_snap_zone(1275, 4, 16),
            Some(SnapZone::TopRight)
        );
        assert_eq!(shell.detect_snap_zone(640, 4, 16), Some(SnapZone::Maximize));
        assert_eq!(shell.detect_snap_zone(4, 400, 16), Some(SnapZone::LeftHalf));
        assert_eq!(
            shell.detect_snap_zone(1275, 400, 16),
            Some(SnapZone::RightHalf)
        );
        assert_eq!(shell.detect_snap_zone(640, 400, 16), None);

        // Snap a to LeftHalf and b to RightHalf -> non-overlapping halves with exact gap
        shell.snap(a, SnapZone::LeftHalf).unwrap();
        shell.snap(b, SnapZone::RightHalf).unwrap();
        let laid = shell.layout().unwrap();
        let ra = laid.iter().find(|p| p.id == a).unwrap();
        let rb = laid.iter().find(|p| p.id == b).unwrap();
        assert_eq!(ra.snap, Some(SnapZone::LeftHalf));
        assert_eq!(rb.snap, Some(SnapZone::RightHalf));
        assert_eq!(rb.rect.x - ra.rect.right(), 8);

        // Unsnap restores pre-snap geometry
        shell.unsnap(a).unwrap();
        assert_eq!(
            shell
                .layout()
                .unwrap()
                .iter()
                .find(|p| p.id == a)
                .unwrap()
                .snap,
            None
        );

        // Pin-on-top elevates `a` above `b` even after focusing `b`
        shell.toggle_pin_on_top(a).unwrap();
        shell.focus_window(b).unwrap();
        let laid = shell.layout().unwrap();
        assert_eq!(laid.last().unwrap().id, a);
        assert!(laid.last().unwrap().pinned);

        // 8-way resize from West edge shifts x and adjusts w while obeying min_w
        shell.move_by(b, 100, 100).unwrap();
        let before_b = shell
            .layout()
            .unwrap()
            .iter()
            .find(|p| p.id == b)
            .unwrap()
            .rect;
        shell.resize_edge(b, ResizeEdge::West, -40, 0).unwrap();
        let after_b = shell
            .layout()
            .unwrap()
            .iter()
            .find(|p| p.id == b)
            .unwrap()
            .rect;
        assert_eq!(after_b.x, before_b.x - 40);
        assert_eq!(after_b.w, before_b.w + 40);
        assert!(shell.resize_edge(b, ResizeEdge::West, 10_000, 0).is_err());

        // Minimize & restore
        shell.minimize(b).unwrap();
        assert!(shell.is_minimized(b).unwrap());
        shell.restore(b).unwrap();
        assert!(!shell.is_minimized(b).unwrap());
    }

    #[test]
    fn f10_hyprland_dwindle_swap_flip_monocle_dividers_and_empty_float() {
        let mut shell = desktop();
        shell.toggle_mode().unwrap();
        assert_eq!(shell.mode().unwrap(), Mode::Dwindle);

        // F13: Opening Settings with floating_centered rule on an EMPTY dwindle workspace succeeds!
        let settings = shell
            .open_with_rule(WindowRule::floating_centered(320, 240, 640, 480), 99)
            .unwrap();
        assert!(shell.is_floating(settings).unwrap());
        assert!(shell.partition().unwrap().is_empty());
        shell.audit().unwrap();

        // Open two tiled windows alongside floating Settings
        let sim = shell.open(200, 150, 1).unwrap();
        let code = shell.open(200, 150, 2).unwrap();
        let parts_before = shell.partition().unwrap();
        assert_eq!(parts_before.len(), 2);
        assert_eq!(parts_before[0].0, sim);
        assert_eq!(parts_before[1].0, code);

        // Swap tiled window right -> positions invert cleanly
        shell.focus_window(sim).unwrap();
        shell.swap_dir(Direction::Right).unwrap();
        let parts_swapped = shell.partition().unwrap();
        assert_eq!(parts_swapped[0].0, code);
        assert_eq!(parts_swapped[1].0, sim);

        // Flip split axis vertical -> horizontal (top/bottom)
        shell.toggle_split_axis(code).unwrap();
        let parts_flipped = shell.partition().unwrap();
        assert_eq!(parts_flipped[0].1.x, parts_flipped[1].1.x);
        assert!(parts_flipped[1].1.y > parts_flipped[0].1.y);

        // Interactive gap divider drag
        let divs = shell.dividers().unwrap();
        assert_eq!(divs.len(), 1);
        assert!(!divs[0].vertical);
        let top_h_before = parts_flipped[0].1.h;
        shell.drag_divider(divs[0].anchor_first, 60).unwrap();
        let top_h_after = shell.partition().unwrap()[0].1.h;
        assert!(top_h_after > top_h_before);

        // Monocle toggle maximizes tiled window to content rect without destroying tree
        shell.maximize(code).unwrap();
        assert_eq!(shell.monocle().unwrap(), Some(code));
        let laid = shell.layout().unwrap();
        let mono_placed = laid.iter().find(|p| p.id == code).unwrap();
        assert!(mono_placed.maximized);
        assert_eq!(mono_placed.rect.w, 1280 - 16);
        assert_eq!(mono_placed.rect.h, 800 - 16);
        // Floating Settings still sits above the monocle pane
        assert_eq!(laid.last().unwrap().id, settings);
        shell.maximize(code).unwrap();
        assert_eq!(shell.monocle().unwrap(), None);

        // Re-tiling Settings brings it into the dwindle partition cleanly
        shell.toggle_float(settings).unwrap();
        assert!(!shell.is_floating(settings).unwrap());
        assert_eq!(shell.partition().unwrap().len(), 3);
        shell.audit().unwrap();
    }

    #[test]
    fn f13_workspaces_1_to_9_and_non_empty_shrink_guard() {
        let mut shell = desktop();
        assert_eq!(shell.workspace_count(), 3);
        shell.set_workspace_count(6).unwrap();
        assert_eq!(shell.workspace_count(), 6);
        shell.switch_workspace(WorkspaceId::SIX).unwrap();
        let w = shell.open(100, 100, 42).unwrap();
        // Shrinking below workspace 6 must fail while workspace 6 holds a window
        assert_eq!(
            shell.set_workspace_count(3),
            Err(WmError::WorkspaceNotEmpty(WorkspaceId::SIX))
        );
        shell.move_to_workspace(w, WorkspaceId::ONE).unwrap();
        shell.set_workspace_count(3).unwrap();
        assert_eq!(shell.active_workspace(), WorkspaceId::THREE);
        assert_eq!(shell.workspace_count(), 3);
    }

    #[test]
    fn fuzz_empty_and_fixed_corpus_are_clean() {
        let (_shell, report) = fuzz_operation_sequence(&[]);
        assert!(report.violations.is_empty());
        assert_eq!(report.applied, 0);
        let _ = clean(&[0, 10, 0, 140, 18, 8, 0, 1, 14, 1, 15, 0, 13, 0, 1, 0]);
    }

    fn same_except_active_mode(before: &Shell, after: &Shell) -> bool {
        if before.bounds != after.bounds
            || before.gaps_in != after.gaps_in
            || before.gaps_out != after.gaps_out
            || before.visible_w != after.visible_w
            || before.visible_h != after.visible_h
            || before.active != after.active
            || before.next_id != after.next_id
        {
            return false;
        }
        before.workspaces.iter().all(|(id, left)| {
            let Some(right) = after.workspaces.get(id) else {
                return false;
            };
            let mode_ok = if *id == before.active {
                left.mode != right.mode
            } else {
                left.mode == right.mode
            };
            mode_ok
                && left.windows == right.windows
                && left.tree == right.tree
                && left.focus == right.focus
                && left.monocle == right.monocle
                && left.z_order == right.z_order
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn i1_tiled_leaves_do_not_overlap(bytes in byte_vec(any::<u8>(), 0..64usize)) {
            let shell = clean(&bytes);
            prop_assert!(shell.check_i1().is_ok());
        }

        #[test]
        fn i2_tiled_area_is_conserved(bytes in byte_vec(any::<u8>(), 0..64usize)) {
            let shell = clean(&bytes);
            prop_assert!(shell.check_i2().is_ok());
        }

        #[test]
        fn i3_minimum_sizes_hold_and_refusal_is_stable(bytes in byte_vec(any::<u8>(), 0..64usize)) {
            let shell = clean(&bytes);
            prop_assert!(shell.check_i3().is_ok());
            let mut refused = shell.clone();
            prop_assert!(refused.open(10_000, 10_000, 1).is_err());
            prop_assert_eq!(refused, shell);
        }

        #[test]
        fn i4_mode_toggle_preserves_window_set(bytes in byte_vec(any::<u8>(), 0..64usize)) {
            let mut shell = clean(&bytes);
            let before = shell.clone();
            prop_assert!(shell.toggle_mode().is_ok());
            prop_assert!(same_except_active_mode(&before, &shell));
            prop_assert!(shell.toggle_mode().is_ok());
            prop_assert_eq!(shell, before);
        }

        #[test]
        fn i5_operation_sequence_is_deterministic(bytes in byte_vec(any::<u8>(), 0..64usize)) {
            let (left, left_report) = fuzz_operation_sequence(&bytes);
            let (right, right_report) = fuzz_operation_sequence(&bytes);
            prop_assert!(left_report.violations.is_empty());
            prop_assert!(right_report.violations.is_empty());
            prop_assert_eq!(left, right);
        }

        #[test]
        fn i6_close_preserves_unrelated_subtree(bytes in byte_vec(any::<u8>(), 0..64usize)) {
            let shell = clean(&bytes);
            let tree = shell.active_ws().unwrap().tree.clone();
            let Some(tree) = tree else {
                return Ok(());
            };
            let mut leaves = Vec::new();
            tree.leaves(&mut leaves);
            for id in leaves {
                let Some(sibling) = sibling_subtree(&tree, id) else {
                    continue;
                };
                let mut next = shell.clone();
                if next.close(id).is_ok() {
                    let after = next.active_ws().unwrap().tree.clone();
                    let Some(after) = after else {
                        prop_assert!(false, "close removed the whole tree while a sibling existed");
                        continue;
                    };
                    prop_assert!(contains_subtree(&after, &sibling));
                    prop_assert!(next.audit().is_ok());
                } else {
                    prop_assert_eq!(&next, &shell);
                }
            }
        }
    }
}
