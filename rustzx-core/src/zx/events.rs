use bitflags::bitflags;

bitflags! {
    /// Emulation events
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct EmulationEvents: u8 {
        /// Set when tape fast load trigger is detected
        const TAPE_FAST_LOAD_TRIGGER_DETECTED = 0b00000001;
        /// Set when PC breakpoint is reached
        const PC_BREAKPOINT = 0b00000010;
    }
}

impl EmulationEvents {
    pub fn take(&mut self) -> Self {
        core::mem::take(self)
    }
}
