//! `EffectiveFlags`: which fields of a command the unit should apply.

use bitflags::bitflags;

bitflags! {
    /// Fields of an air-to-air (`SetAta`) command that the unit should apply.
    ///
    /// A command carries the whole unit state, so these flags tell the server which fields
    /// changed. The server reports `0` again once the unit has applied the command.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct EffectiveFlags: u64 {
        /// `Power`.
        const POWER = 1;
        /// `OperationMode`.
        const OPERATION_MODE = 2;
        /// `SetTemperature`.
        const SET_TEMPERATURE = 4;
        /// `SetFanSpeed`.
        const FAN_SPEED = 8;
        /// `VaneVertical`.
        const VANE_VERTICAL = 16;
        /// `ProhibitPower`, `ProhibitOperationMode`, `ProhibitSetTemperature`: locks the local
        /// remote controller.
        const PROHIBIT_CONTROLLER = 64;
        /// `VaneHorizontal`.
        const VANE_HORIZONTAL = 256;
    }
}

#[cfg(test)]
mod tests {
    use super::EffectiveFlags;

    #[test]
    fn bits_match_the_api() {
        let pairs = [
            (EffectiveFlags::POWER, 1),
            (EffectiveFlags::OPERATION_MODE, 2),
            (EffectiveFlags::SET_TEMPERATURE, 4),
            (EffectiveFlags::FAN_SPEED, 8),
            (EffectiveFlags::VANE_VERTICAL, 16),
            (EffectiveFlags::PROHIBIT_CONTROLLER, 64),
            (EffectiveFlags::VANE_HORIZONTAL, 256),
        ];
        for (flag, bits) in pairs {
            assert_eq!(flag.bits(), bits, "{flag:?}");
        }
    }

    #[test]
    fn several_changes_combine() {
        let flags = EffectiveFlags::SET_TEMPERATURE | EffectiveFlags::FAN_SPEED;
        assert_eq!(flags.bits(), 12);
    }
}
