use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("EscrowVault11111111111111111111111111111111");

#[program]
pub mod escrow_vault {
    use super::*;

    /// Initializes a milestone escrow vault for an engagement between client and worker.
    pub fn initialize_escrow(
        ctx: Context<InitializeEscrow>,
        escrow_id: u64,
        total_amount: u64,
        milestone_count: u8,
        deadline: i64,
    ) -> Result<()> {
        require!(total_amount > 0, EscrowError::InvalidAmount);
        require!(milestone_count > 0 && milestone_count <= 10, EscrowError::InvalidMilestoneCount);
        require!(deadline > Clock::get()?.unix_timestamp, EscrowError::InvalidDeadline);

        let escrow = &mut ctx.accounts.escrow_account;
        escrow.escrow_id = escrow_id;
        escrow.client = ctx.accounts.client.key();
        escrow.worker = ctx.accounts.worker.key();
        escrow.mint = ctx.accounts.mint.key();
        escrow.vault = ctx.accounts.vault_token_account.key();
        escrow.total_amount = total_amount;
        escrow.released_amount = 0;
        escrow.milestone_count = milestone_count;
        escrow.completed_milestones = 0;
        escrow.deadline = deadline;
        escrow.is_cancelled = false;
        escrow.bump = ctx.bumps.escrow_account;

        // Transfer funds from client token account to vault PDA
        let cpi_accounts = Transfer {
            from: ctx.accounts.client_token_account.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.client.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, total_amount)?;

        emit!(EscrowInitialized {
            escrow_id,
            client: escrow.client,
            worker: escrow.worker,
            total_amount,
            deadline,
        });

        Ok(())
    }

    /// Releases a portion of funds upon verified completion of a milestone.
    pub fn release_milestone(
        ctx: Context<ReleaseMilestone>,
        milestone_index: u8,
        milestone_amount: u64,
        deliverable_hash: [u8; 32],
    ) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;
        require!(!escrow.is_cancelled, EscrowError::EscrowCancelled);
        require!(milestone_index == escrow.completed_milestones, EscrowError::MilestoneSequenceError);
        require!(
            escrow.released_amount.checked_add(milestone_amount).unwrap() <= escrow.total_amount,
            EscrowError::ExceedsTotalDeposit
        );

        escrow.completed_milestones += 1;
        escrow.released_amount = escrow.released_amount.checked_add(milestone_amount).unwrap();

        // Transfer milestone payout from vault PDA to worker token account
        let escrow_id_bytes = escrow.escrow_id.to_le_bytes();
        let seeds = &[
            b"escrow",
            escrow.client.as_ref(),
            escrow_id_bytes.as_ref(),
            &[escrow.bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault_token_account.to_account_info(),
            to: ctx.accounts.worker_token_account.to_account_info(),
            authority: escrow.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::transfer(cpi_ctx, milestone_amount)?;

        emit!(MilestoneReleased {
            escrow_id: escrow.escrow_id,
            milestone_index,
            released_amount: milestone_amount,
            deliverable_hash,
        });

        Ok(())
    }

    /// Cancels escrow after deadline or upon mutual agreement, refunding remaining deposit.
    pub fn cancel_and_refund(ctx: Context<CancelAndRefund>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow_account;
        let clock = Clock::get()?;
        require!(!escrow.is_cancelled, EscrowError::EscrowCancelled);

        // Client can refund if expired; otherwise mutual cancellation requires both signers
        let is_expired = clock.unix_timestamp > escrow.deadline;
        if !is_expired {
            require!(
                ctx.accounts.worker.is_some(),
                EscrowError::UnauthorizedCancellation
            );
        }

        let remaining = escrow.total_amount.checked_sub(escrow.released_amount).unwrap();
        escrow.is_cancelled = true;

        if remaining > 0 {
            let escrow_id_bytes = escrow.escrow_id.to_le_bytes();
            let seeds = &[
                b"escrow",
                escrow.client.as_ref(),
                escrow_id_bytes.as_ref(),
                &[escrow.bump],
            ];
            let signer_seeds = &[&seeds[..]];

            let cpi_accounts = Transfer {
                from: ctx.accounts.vault_token_account.to_account_info(),
                to: ctx.accounts.client_token_account.to_account_info(),
                authority: escrow.to_account_info(),
            };
            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                cpi_accounts,
                signer_seeds,
            );
            token::transfer(cpi_ctx, remaining)?;
        }

        emit!(EscrowCancelled {
            escrow_id: escrow.escrow_id,
            refunded_amount: remaining,
        });

        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(escrow_id: u64)]
pub struct InitializeEscrow<'info> {
    #[account(mut)]
    pub client: Signer<'info>,

    /// CHECK: Worker wallet receiving milestone payments
    pub worker: AccountInfo<'info>,

    pub mint: Account<'info, Mint>,

    #[account(
        mut,
        constraint = client_token_account.owner == client.key(),
        constraint = client_token_account.mint == mint.key()
    )]
    pub client_token_account: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = client,
        space = EscrowAccount::LEN,
        seeds = [b"escrow", client.key().as_ref(), escrow_id.to_le_bytes().as_ref()],
        bump
    )]
    pub escrow_account: Account<'info, EscrowAccount>,

    #[account(
        init,
        payer = client,
        token::mint = mint,
        token::authority = escrow_account,
        seeds = [b"vault", escrow_account.key().as_ref()],
        bump
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct ReleaseMilestone<'info> {
    #[account(
        mut,
        constraint = escrow_account.client == client.key() || escrow_account.worker == worker.key()
    )]
    pub client: Signer<'info>,

    /// CHECK: Verified against escrow worker field
    #[account(constraint = worker.key() == escrow_account.worker)]
    pub worker: AccountInfo<'info>,

    #[account(
        mut,
        seeds = [b"escrow", escrow_account.client.as_ref(), escrow_account.escrow_id.to_le_bytes().as_ref()],
        bump = escrow_account.bump
    )]
    pub escrow_account: Account<'info, EscrowAccount>,

    #[account(
        mut,
        seeds = [b"vault", escrow_account.key().as_ref()],
        bump
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = worker_token_account.owner == worker.key(),
        constraint = worker_token_account.mint == escrow_account.mint
    )]
    pub worker_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct CancelAndRefund<'info> {
    #[account(mut, constraint = escrow_account.client == client.key())]
    pub client: Signer<'info>,

    pub worker: Option<Signer<'info>>,

    #[account(
        mut,
        seeds = [b"escrow", escrow_account.client.as_ref(), escrow_account.escrow_id.to_le_bytes().as_ref()],
        bump = escrow_account.bump
    )]
    pub escrow_account: Account<'info, EscrowAccount>,

    #[account(
        mut,
        seeds = [b"vault", escrow_account.key().as_ref()],
        bump
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = client_token_account.owner == client.key(),
        constraint = client_token_account.mint == escrow_account.mint
    )]
    pub client_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[account]
pub struct EscrowAccount {
    pub escrow_id: u64,
    pub client: Pubkey,
    pub worker: Pubkey,
    pub mint: Pubkey,
    pub vault: Pubkey,
    pub total_amount: u64,
    pub released_amount: u64,
    pub milestone_count: u8,
    pub completed_milestones: u8,
    pub deadline: i64,
    pub is_cancelled: bool,
    pub bump: u8,
}

impl EscrowAccount {
    pub const LEN: usize = 8 + // discriminator
        8 +  // escrow_id
        32 + // client
        32 + // worker
        32 + // mint
        32 + // vault
        8 +  // total_amount
        8 +  // released_amount
        1 +  // milestone_count
        1 +  // completed_milestones
        8 +  // deadline
        1 +  // is_cancelled
        1;   // bump
}

#[event]
pub struct EscrowInitialized {
    pub escrow_id: u64,
    pub client: Pubkey,
    pub worker: Pubkey,
    pub total_amount: u64,
    pub deadline: i64,
}

#[event]
pub struct MilestoneReleased {
    pub escrow_id: u64,
    pub milestone_index: u8,
    pub released_amount: u64,
    pub deliverable_hash: [u8; 32],
}

#[event]
pub struct EscrowCancelled {
    pub escrow_id: u64,
    pub refunded_amount: u64,
}

#[error_code]
pub enum EscrowError {
    #[msg("Total deposit must be greater than zero")]
    InvalidAmount,
    #[msg("Milestone count must be between 1 and 10")]
    InvalidMilestoneCount,
    #[msg("Deadline must be in the future")]
    InvalidDeadline,
    #[msg("Escrow has already been cancelled")]
    EscrowCancelled,
    #[msg("Milestone submitted out of sequence")]
    MilestoneSequenceError,
    #[msg("Requested release amount exceeds remaining total deposit")]
    ExceedsTotalDeposit,
    #[msg("Unauthorized cancellation before deadline without worker co-signature")]
    UnauthorizedCancellation,
}
