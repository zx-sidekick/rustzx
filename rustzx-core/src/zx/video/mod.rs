//! Module contains all screen-rendering platform-independent
//! types and functions
#[cfg(feature = "precise-border")]
pub(crate) mod border;
pub(crate) mod screen;

pub mod colors;

/// How much border the border frame buffer holds around the 256×192 screen
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BorderSize {
    /// 32 pixels left and right, 24 lines above and below: 320×240
    #[default]
    Standard,
    /// 32 pixels left and right, 32 lines above and below: 320×256
    Tall,
}

impl BorderSize {
    /// Border character rows above (and below) the screen
    pub(crate) const fn rows(self) -> usize {
        match self {
            Self::Standard => 3,
            Self::Tall => 4,
        }
    }

    /// Lines of border above the screen, where the screen starts in the border buffer
    pub const fn canvas_y(self) -> usize {
        self.rows() * 8
    }

    /// The border buffer's height in pixels
    pub const fn height(self) -> usize {
        crate::zx::constants::CANVAS_HEIGHT + self.canvas_y() * 2
    }
}
