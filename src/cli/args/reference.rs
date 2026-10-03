//! `hw ref` — reference data the other endpoints point to by id.

use clap::Subcommand;

/// Reference data (no parameters; every user with a token may read it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum RefCommand {
    /// Business settings: booking grid step, opening hours, business-day start, time zone
    Settings,
    /// Tables available for booking, in interface order
    Tables,
    /// Payment methods, in checkout order
    PaymentMethods,
    /// Active employees, sorted by name
    Employees,
    /// Services (hookah types and related items)
    Services,
    /// Bonus program levels, from the entry level up
    BonusLevels,
}
