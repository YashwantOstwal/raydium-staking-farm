use crate::{states::*,error::ErrorCode};

use anchor_lang::prelude::*;
use anchor_spl::{token_interface::{Mint,TokenAccount,TokenInterface,transfer_checked,TransferChecked}};

#[derive(Accounts)]
pub struct WithdrawReward<'info> {

    pub authority: Signer<'info>,

    pub staking_mint:Box< InterfaceAccount<'info,Mint>>,

    #[account(
        mut,
        has_one = authority,
        has_one = staking_mint,
        seeds = [Farm::STATIC_SEED,staking_mint.key().as_ref()],
        bump = farm.bump
    )]
    pub farm:Box< Account<'info,Farm>>,

    #[account(
        mint::token_program = reward_mint_program,
    )]
    pub reward_mint:Box<InterfaceAccount<'info,Mint>>,

    #[account(
        mut,
        token::mint = reward_mint,
        token::authority = authority,
        token::token_program = reward_mint_program,
    )]
    pub authority_reward_token: Box<InterfaceAccount<'info,TokenAccount>>,

    #[account(
        mut,
        associated_token::mint = reward_mint,
        associated_token::authority = farm,
        associated_token::token_program = reward_mint_program,
    )]
    pub reward_vault: Box<InterfaceAccount<'info,TokenAccount>>,

    pub reward_mint_program:Interface<'info,TokenInterface>,
}

// To be invoked only after the reward stream is ended and every staker have had a chance to harvest their latest rewards.
// Do not call it early. or else harvest on behalf of all the stakers and then call withdraw.

pub fn handle_withdraw_reward(ctx:Context<WithdrawReward>,reward_stream_idx:u8)-> Result<()> {

    let farm = &mut ctx.accounts.farm;
    farm.update()?;

    require!(reward_stream_idx < farm.reward_streams_count,ErrorCode::ReferencedRewardStreamInvalid);
    require!(ctx.accounts.reward_mint.key() == farm.reward_streams[reward_stream_idx as usize].reward_mint,ErrorCode::MismatchingAccounts);

    require!(farm.reward_streams[reward_stream_idx as usize].status == RewardStreamStatus::Ended,ErrorCode::RewardStreamIsRunning);
    
    if ctx.accounts.reward_vault.amount > 0 {
        let farm_seeds:&[&[u8]] = &[Farm::STATIC_SEED,farm.staking_mint.as_ref(),&[farm.bump]];
        let signer_seeds = [&farm_seeds[..]];

        let transfer_ctx = CpiContext::new(ctx.accounts.reward_mint_program.key(), TransferChecked {
            from:ctx.accounts.reward_vault.to_account_info(),
            to:ctx.accounts.authority_reward_token.to_account_info(),
            mint:ctx.accounts.reward_mint.to_account_info(),
            authority:farm.to_account_info()
        }).with_signer(&signer_seeds);

        transfer_checked(transfer_ctx, ctx.accounts.reward_vault.amount, ctx.accounts.reward_mint.decimals)?;
    }
    farm.reward_streams[reward_stream_idx as usize].rewards_left_x64 = 0;
    Ok(())
}
