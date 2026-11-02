use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use crate::state::PoolRegistry;
use crate::errors::FlintError;

#[derive(Accounts)]
pub struct WithdrawLiquidity<'info> {
    #[account(
        mut,
        address = pool.maker_authority @ FlintError::Unauthorized
    )]
    pub maker_authority: Signer<'info>,

    #[account(
        mut,
        has_one = maker_authority,
        has_one = vault_a,
        has_one = vault_b
    )]
    pub pool: Box<Account<'info, PoolRegistry>>,

    #[account(mut)]
    pub vault_a: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub vault_b: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub destination_a: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub destination_b: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<WithdrawLiquidity>, amount_a: u64, amount_b: u64) -> Result<()> {
    let pool = &ctx.accounts.pool;
    let maker_key = pool.maker_authority.key();
    let mint_a_key = pool.mint_a.key();
    let mint_b_key = pool.mint_b.key();

    let seeds: &[&[&[u8]]] = &[&[
        b"pool",
        maker_key.as_ref(),
        mint_a_key.as_ref(),
        mint_b_key.as_ref(),
        &[pool.pool_bump],
    ]];

    if amount_a > 0 {
        let cpi_a = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault_a.to_account_info(),
                to: ctx.accounts.destination_a.to_account_info(),
                authority: ctx.accounts.pool.to_account_info(),
            },
            seeds,
        );
        token::transfer(cpi_a, amount_a)?;
    }

    if amount_b > 0 {
        let cpi_b = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault_b.to_account_info(),
                to: ctx.accounts.destination_b.to_account_info(),
                authority: ctx.accounts.pool.to_account_info(),
            },
            seeds,
        );
        token::transfer(cpi_b, amount_b)?;
    }

    msg!("Withdrawn liquidity: A={}, B={}", amount_a, amount_b);
    Ok(())
}