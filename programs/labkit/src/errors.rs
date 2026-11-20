use anchor_lang::prelude::*;

#[error_code]
pub enum LabkitError {
    #[msg("Only the program upgrade authority can initialize LabKit.")]
    NotUpgradeAuthority,
    #[msg("Crank fee is above the maximum.")]
    CrankFeeTooHigh,
    #[msg("Treasury cannot be the default public key.")]
    InvalidTreasury,
    #[msg("Treasury token account is not owned by the configured treasury.")]
    TreasuryMismatch,
    #[msg("Only the pending authority can accept.")]
    NotPendingAuthority,
    #[msg("There is nothing to split.")]
    NothingToSplit,
    #[msg("Arithmetic overflow.")]
    Overflow,
    #[msg("A share in bps is above 100%.")]
    InvalidBps,
    #[msg("Pool state is not a LaunchLab pool for this mint.")]
    BadPoolState,
    #[msg("The LaunchLab pool's creator is not this launch's creator PDA.")]
    PoolCreatorMismatch,
    #[msg("Recipient cannot be the default public key.")]
    InvalidRecipient,
    #[msg("Recipient token account is not owned by the launch recipient.")]
    RecipientMismatch,
    #[msg("The pool is not on this launch's platform.")]
    PlatformMismatch,
    #[msg("Pool state does not belong to this launch.")]
    PoolMismatch,
    #[msg("Platform settings are out of range.")]
    InvalidPlatform,
    #[msg("Unknown or unavailable kit, or kit settings out of range.")]
    InvalidKit,
    #[msg("The launch has no LaunchLab pool yet.")]
    NotRegistered,
    #[msg("The launch is already registered.")]
    AlreadyRegistered,
    #[msg("LabKit launches trade against SOL.")]
    QuoteNotSol,
    #[msg("This kit does not do that.")]
    WrongKit,
    #[msg("The bonding curve is not trading.")]
    CurveNotTrading,
    #[msg("The launch has not moved to Raydium.")]
    NotMigrated,
    #[msg("The curve has not completed yet.")]
    NotGraduated,
    #[msg("The kit's balance is under the minimum buy.")]
    BelowMinimum,
    #[msg("The price is above the floor line.")]
    AboveFloor,
    #[msg("The CPMM pool does not belong to this launch.")]
    BadAmmPool,
    #[msg("The buy spent more than planned.")]
    Overspend,
    #[msg("The amount must be above zero.")]
    ZeroAmount,
    #[msg("That is more than is staked.")]
    InsufficientStake,
    #[msg("There are no rewards to claim.")]
    NothingToClaim,
    #[msg("The draw tree does not match.")]
    BadDrawTree,
    #[msg("A draw is in progress. Try again in a moment.")]
    DrawInProgress,
    #[msg("No draw is waiting to be settled.")]
    NoDrawPending,
    #[msg("The next draw is not due yet.")]
    DrawNotDue,
    #[msg("The pot is under the minimum for a draw.")]
    PotTooSmall,
    #[msg("Nobody is staked.")]
    NoStakers,
    #[msg("The VRF request cost more than a draw may refund.")]
    DrawTooCostly,
    #[msg("The randomness has not arrived yet.")]
    NotFulfilled,
    #[msg("The randomness has arrived, so the draw must be settled.")]
    AlreadyFulfilled,
    #[msg("The draw has not timed out yet.")]
    NotTimedOut,
    #[msg("A picked position is missing or wrong.")]
    BadCandidate,
    #[msg("The kit already bought in this slot.")]
    ExecutedThisSlot,
    #[msg("LabKit launches use an SPL Token mint, not Token-2022.")]
    UnsupportedBaseToken,
    #[msg("Raydium creator fees must be taken in SOL only.")]
    CreatorFeeNotQuote,
}
