use crate::tui::Component;

// =============================================================================
// Overlay types — matching pi's packages/tui/src/tui.ts
// =============================================================================

/// Anchor position for overlays
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum OverlayAnchor {
    #[default]
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    TopCenter,
    BottomCenter,
    LeftCenter,
    RightCenter,
}

/// Margin configuration for overlays
#[derive(Debug, Clone, Copy, Default)]
pub struct OverlayMargin {
    pub top: usize,
    pub right: usize,
    pub bottom: usize,
    pub left: usize,
}

impl OverlayMargin {
    pub fn uniform(value: usize) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
}

/// Value that can be absolute (number) or percentage (string like "50%").
/// For simplicity in Rust, we represent percentage as f64 (0.0..=100.0).
#[derive(Debug, Clone, Copy)]
pub enum SizeValue {
    Absolute(usize),
    Percent(f64),
}

impl SizeValue {
    pub fn resolve(&self, reference: usize) -> usize {
        match self {
            SizeValue::Absolute(v) => *v,
            SizeValue::Percent(p) => {
                let v = (reference as f64 * p / 100.0).floor() as usize;
                v.max(1)
            }
        }
    }
}

/// Options for overlay positioning and sizing.
#[derive(Debug, Clone, Default)]
pub struct OverlayOptions {
    // === Sizing ===
    /// Width in columns, or percentage of terminal width
    pub width: Option<SizeValue>,
    /// Minimum width in columns
    pub min_width: Option<usize>,
    /// Maximum height in rows, or percentage of terminal height
    pub max_height: Option<SizeValue>,

    // === Positioning - anchor-based ===
    /// Anchor point for positioning (default: Center)
    pub anchor: Option<OverlayAnchor>,
    /// Horizontal offset from anchor position (positive = right)
    pub offset_x: Option<isize>,
    /// Vertical offset from anchor position (positive = down)
    pub offset_y: Option<isize>,

    // === Positioning - percentage or absolute ===
    /// Row position: absolute number, or percentage from top
    pub row: Option<SizeValue>,
    /// Column position: absolute number, or percentage from left
    pub col: Option<SizeValue>,

    // === Margin from terminal edges ===
    pub margin: Option<OverlayMargin>,

    // === Visibility ===
    /// If true, don't capture keyboard focus when shown
    pub non_capturing: bool,
}

/// Internal entry in the overlay stack
pub struct OverlayEntry {
    pub component: Box<dyn Component>,
    pub options: OverlayOptions,
    /// Whether this overlay is temporarily hidden
    pub hidden: bool,
    /// Order for compositing (higher = on top)
    pub focus_order: u64,
    /// Unique ID for this overlay
    pub id: u64,
    /// Focus target that was active before this overlay was shown.
    /// Restored when the overlay is dismissed.
    pub pre_focus: crate::tui::FocusTarget,
}

/// Resolved overlay layout
#[derive(Debug, Clone)]
pub struct OverlayLayout {
    pub width: usize,
    pub row: usize,
    pub col: usize,
    pub max_height: Option<usize>,
}
