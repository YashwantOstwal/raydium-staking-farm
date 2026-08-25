use anchor_lang::prelude::*;

use anchor_spl::{associated_token::{self, get_associated_token_address_with_program_id, Create}, token, token_2022};

use litesvm::{LiteSVM,types::{TransactionMetadata,FailedTransactionMetadata}};
use litesvm_token::{
    get_spl_account, spl_token::{native_mint::DECIMALS, state::Account as TokenAccount}, CreateAccount, CreateAssociatedTokenAccount, CreateMint, MintTo, Transfer,TOKEN_ID
};
use raydium_farm::{create_farm, Farm, RewardStreamArgs, RewardStreamStatus, UserLedger};
use sha2::{Sha256, Digest};
use solana_sdk::{
    
    account::Account, clock::{self, Clock}, message::{AccountMeta, Instruction}, native_token::LAMPORTS_PER_SOL, pubkey::Pubkey, signature::{read_keypair_file, Keypair, Signer}, sysvar::Sysvar, transaction::Transaction

};

pub struct RewardStream<'a> {
    pub reward_mint:&'a Pubkey,
    pub open_time:i64,
    pub end_time:i64,
    pub emission_per_second_x64: u128
}

pub struct CreateFarmIxn<'a> {
    pub creator:&'a Keypair,
    pub staking_mint:&'a Pubkey,
    pub reward_streams: [Option<RewardStream<'a>>;5]
}
const RAYDIUM_PROGRAM_ID: Pubkey = raydium_farm::ID_CONST;
// read_keypair_file("/home/yashwant/Desktop/web3/raydium-farm/target/deploy/raydium_farm-keypair.json").unwrap().pubkey();

pub fn create_farm(svm: &mut LiteSVM,CreateFarmIxn {creator,staking_mint,reward_streams} :CreateFarmIxn) -> std::result::Result<TransactionMetadata, FailedTransactionMetadata> {

    let mut hasher = Sha256::new();
    hasher.update("global:create_farm");
    let hash = hasher.finalize();

    let mut create_farm_discriminator = [0u8;8];
    create_farm_discriminator.copy_from_slice(&hash[..8]);

    let mut create_farm_ixn_data = create_farm_discriminator.to_vec();

    let farm_seeds = &[b"farm",staking_mint.as_ref()];

    let farm_pda = Pubkey::find_program_address(farm_seeds, &RAYDIUM_PROGRAM_ID).0;

    let staking_mint_account = svm.get_account(&staking_mint).unwrap();
    let staking_mint_program = staking_mint_account.owner;
    
    let staking_token_vault = get_associated_token_address_with_program_id(&farm_pda, staking_mint, &staking_mint_program);

    let mut accounts =  vec![
            AccountMeta::new(creator.pubkey(),true),
            AccountMeta::new_readonly(*staking_mint,false),
            AccountMeta::new(staking_token_vault,false),
            AccountMeta::new(farm_pda,false),
            AccountMeta::new_readonly(system_program::ID,false),
            AccountMeta::new_readonly(token::ID,false),
            AccountMeta::new_readonly(token_2022::ID,false),
            AccountMeta::new_readonly(associated_token::ID,false),
        ];


    // Remaining accounts.
    for i in 0..5  {
        if let Some(RewardStream {reward_mint,open_time,end_time,emission_per_second_x64}) = reward_streams[i as usize] {

            accounts.push(AccountMeta::new_readonly(*reward_mint,false));

            let reward_mint_program = svm.get_account(reward_mint).unwrap().owner;
            let reward_vault = get_associated_token_address_with_program_id(&farm_pda, reward_mint, &reward_mint_program);
            accounts.push(AccountMeta::new(reward_vault,false));
    
            let creator_reward_token = get_associated_token_address_with_program_id(&creator.pubkey(), reward_mint, &reward_mint_program);
            accounts.push(AccountMeta::new(creator_reward_token,false));

            
            // ixn data
            create_farm_ixn_data.extend_from_slice(&1u8.to_le_bytes());
            create_farm_ixn_data.extend_from_slice(&open_time.to_le_bytes());
            create_farm_ixn_data.extend_from_slice(&end_time.to_le_bytes());
            create_farm_ixn_data.extend_from_slice(&emission_per_second_x64.to_le_bytes());
        }else {
            create_farm_ixn_data.extend_from_slice(&0u8.to_le_bytes());
            create_farm_ixn_data.extend_from_slice(&0i64.to_le_bytes());
            create_farm_ixn_data.extend_from_slice(&0i64.to_le_bytes());
            create_farm_ixn_data.extend_from_slice(&0u128.to_le_bytes());
        }
    }
    let create_farm_ixn = Instruction {
        program_id:RAYDIUM_PROGRAM_ID,
        accounts,
        data: create_farm_ixn_data
    };

    let tx = Transaction::new_signed_with_payer(&[create_farm_ixn], Some(&creator.pubkey()), &[&creator], svm.latest_blockhash());

    svm.send_transaction(tx)
}


// Use this to verify the status immediately after updating the farm. i.e, farm_data.last_updated_time == block_timestamp 
pub fn verify_updated_farm_status(svm:&LiteSVM,farm_data:&Farm,reward_idx:u8) {
    assert!(reward_idx < 5);

    // As the farm state is lazy updated, verify it only after updating.
    let block_timestamp = (svm.get_sysvar::<Clock>()).unix_timestamp;
    assert_eq!(farm_data.last_updated_time,block_timestamp);

    let farm_reward =  &farm_data.reward_streams[reward_idx as usize]; 
    match farm_reward.status {
        RewardStreamStatus::Unused => {
            assert!(block_timestamp < farm_reward.open_time);
        } 
        RewardStreamStatus::Running => {
            assert!(farm_reward.open_time <= block_timestamp && block_timestamp <= farm_reward.end_time);
        } 
        RewardStreamStatus::Ended => {
            assert!(farm_reward.end_time < block_timestamp);
        } 
    };
}

pub struct StakeIxn<'a>{
    pub staker:&'a Keypair,
    pub staking_mint:&'a Pubkey,
    pub staker_staking_token:&'a Pubkey,
    pub reward_tokens:&'a Vec<Pubkey>,
    pub deposit_amount:u64
}
pub fn derive_farm_pda(staking_mint:&Pubkey) -> Pubkey{
    let staking_mint_slice = staking_mint.to_bytes();
    let farm_seeds:&[&[u8]] = &[raydium_farm::Farm::STATIC_SEED,staking_mint_slice.as_ref()]; 
    let (farm_pda,_farm_bump) = Pubkey::find_program_address(farm_seeds, &RAYDIUM_PROGRAM_ID);
    farm_pda
}

pub fn derive_user_ledger_pda(farm_pda:&Pubkey,user:&Pubkey) -> (Pubkey,u8) {
    let farm_pda_slice = farm_pda.to_bytes();
    let user_pda_slice = user.to_bytes();
    let user_ledger_seeds:&[&[u8]] = &[raydium_farm::UserLedger::STATIC_SEED,farm_pda_slice.as_ref(),user_pda_slice.as_ref()];
    Pubkey::find_program_address(user_ledger_seeds, &RAYDIUM_PROGRAM_ID)
}
pub fn stake(svm:&mut LiteSVM, StakeIxn {
    staker,staking_mint,staker_staking_token,reward_tokens,deposit_amount
}:StakeIxn)
 -> std::result::Result<TransactionMetadata,FailedTransactionMetadata>
  {
    let mut hasher = Sha256::new();
    hasher.update("global:deposit");
    let hash = hasher.finalize();

    let mut deposit_ixn_discriminator = [0u8;8];
    deposit_ixn_discriminator.copy_from_slice(&hash[..8]);

    let mut deposit_ixn_data = deposit_ixn_discriminator.to_vec();
    deposit_ixn_data.extend_from_slice(&deposit_amount.to_le_bytes());

    
    let farm_pda = derive_farm_pda(&staking_mint);
    let (user_ledger,_) = derive_user_ledger_pda(&farm_pda,&staker.pubkey());

    let staking_mint_account = svm.get_account(&staking_mint).unwrap();
    let staking_mint_program = staking_mint_account.owner;
    
    let staking_token_vault = get_associated_token_address_with_program_id(&farm_pda, staking_mint, &staking_mint_program);
    let mut deposit_ixn_accounts = vec![
        AccountMeta::new(staker.pubkey(),true),
        AccountMeta::new_readonly(*staking_mint, false),
        AccountMeta::new(farm_pda,false),
        AccountMeta::new(*staker_staking_token,false),
        AccountMeta::new(staking_token_vault,false),
        AccountMeta::new(user_ledger,false),
        AccountMeta::new_readonly(system_program::ID,false),
        AccountMeta::new_readonly(token::ID,false),
        AccountMeta::new_readonly(token_2022::ID,false),
    ];

    let farm = svm.get_account(&farm_pda).unwrap();
    let farm_data:raydium_farm::Farm = Farm::try_deserialize(&mut farm.data.as_slice()).unwrap();
    assert_eq!(reward_tokens.len(),farm_data.reward_streams_count as usize);

    // Remaining accounts.
    for i in 0..reward_tokens.len() {
        deposit_ixn_accounts.push(AccountMeta::new_readonly(farm_data.reward_streams[i].reward_mint,false));

        let reward_vault = get_associated_token_address_with_program_id(&farm_pda, &farm_data.reward_streams[i].reward_mint, &farm_data.reward_streams[i].reward_mint_program);
        deposit_ixn_accounts.push(AccountMeta::new(reward_vault,false));

        deposit_ixn_accounts.push(AccountMeta::new(reward_tokens[i as usize],false))
    }

    let deposit_ixn = Instruction {
        program_id:RAYDIUM_PROGRAM_ID,
        accounts:deposit_ixn_accounts,
        data:deposit_ixn_data
    };

    let tx = Transaction::new_signed_with_payer(&[deposit_ixn], Some(&staker.pubkey()), &[&staker,], svm.latest_blockhash());

    svm.send_transaction(tx)
}

pub struct UnstakeIxn<'a>{
    pub staker:&'a Keypair,
    pub staking_mint:&'a Pubkey,
    pub staker_staking_token:&'a Pubkey,
    pub reward_tokens:&'a Vec<Pubkey>,
    pub withdraw_amount:u64
}
pub fn unstake(svm:&mut LiteSVM, UnstakeIxn {
    staker,staking_mint,staker_staking_token,reward_tokens,withdraw_amount
}:UnstakeIxn)
 -> std::result::Result<TransactionMetadata,FailedTransactionMetadata>
  {
    let mut hasher = Sha256::new();
    hasher.update("global:withdraw");
    let hash = hasher.finalize();

    let mut withdraw_ixn_discriminator = [0u8;8];
    withdraw_ixn_discriminator.copy_from_slice(&hash[..8]);

    let mut withdraw_ixn_data = withdraw_ixn_discriminator.to_vec();
    withdraw_ixn_data.extend_from_slice(&withdraw_amount.to_le_bytes());

    
    let farm_pda = derive_farm_pda(&staking_mint);
    let (user_ledger,_) = derive_user_ledger_pda(&farm_pda,&staker.pubkey());

    let staking_mint_account = svm.get_account(&staking_mint).unwrap();
    let staking_mint_program = staking_mint_account.owner;

    let staking_token_vault = get_associated_token_address_with_program_id(&farm_pda, staking_mint, &staking_mint_program);
    let mut withdraw_ixn_accounts = vec![
        AccountMeta::new_readonly(staker.pubkey(),true),
        AccountMeta::new_readonly(*staking_mint, false),
        AccountMeta::new(farm_pda,false),
        AccountMeta::new(*staker_staking_token,false),
        AccountMeta::new(staking_token_vault,false),
        AccountMeta::new(user_ledger,false),
        AccountMeta::new_readonly(token::ID,false),
        AccountMeta::new_readonly(token_2022::ID,false),
    ];

    let farm = svm.get_account(&farm_pda).unwrap();
    let farm_data:raydium_farm::Farm = Farm::try_deserialize(&mut farm.data.as_slice()).unwrap();
    assert_eq!(reward_tokens.len(),farm_data.reward_streams_count as usize);

    // Remaining accounts.
    for i in 0..reward_tokens.len() {
        withdraw_ixn_accounts.push(AccountMeta::new_readonly(farm_data.reward_streams[i].reward_mint,false));

        let reward_vault = get_associated_token_address_with_program_id(&farm_pda, &farm_data.reward_streams[i].reward_mint, &farm_data.reward_streams[i].reward_mint_program);
        withdraw_ixn_accounts.push(AccountMeta::new(reward_vault,false));

        withdraw_ixn_accounts.push(AccountMeta::new(reward_tokens[i as usize],false))
    }

    let withdraw_ixn = Instruction {
        program_id:RAYDIUM_PROGRAM_ID,
        accounts:withdraw_ixn_accounts,
        data:withdraw_ixn_data
    };

    let tx = Transaction::new_signed_with_payer(&[withdraw_ixn], Some(&staker.pubkey()), &[&staker,], svm.latest_blockhash());

    svm.send_transaction(tx)
}

pub fn get_farm(svm:&LiteSVM,farm_pda:&Pubkey) -> Farm {
    let farm= svm.get_account(&farm_pda).unwrap();
    Farm::try_deserialize(&mut farm.data.as_slice()).unwrap()
}
pub fn get_user_ledger(svm:&LiteSVM, user_ledger_pda:&Pubkey)-> UserLedger{
    let  user_ledger = svm.get_account(user_ledger_pda).unwrap();
    UserLedger::try_deserialize( &mut user_ledger.data.as_slice()).unwrap()
}

pub struct HarvestIxn<'a>{
    pub staker:&'a Keypair,
    pub staking_mint:&'a Pubkey,
    pub reward_tokens:&'a Vec<Pubkey>,
}

pub fn harvest(svm:&mut LiteSVM,HarvestIxn {
    staker,staking_mint,reward_tokens
}:HarvestIxn) -> std::result::Result<TransactionMetadata,FailedTransactionMetadata> {

    let mut hasher = Sha256::new();
    hasher.update("global:harvest");
    let hash = hasher.finalize();

    let mut harvest_ixn_discriminator = [0u8;8];
    harvest_ixn_discriminator.copy_from_slice(&hash[..8]);

    let farm_pda = derive_farm_pda(&staking_mint);
    let (user_ledger_pda,_) = derive_user_ledger_pda(&farm_pda,&staker.pubkey());


    let harvest_ixn_data = harvest_ixn_discriminator.to_vec();
    let mut harvest_ixn_accounts = vec![
        AccountMeta::new_readonly(staker.pubkey(),false),
        AccountMeta::new_readonly(*staking_mint, false),
        AccountMeta::new(farm_pda,false),
        AccountMeta::new(user_ledger_pda,false),
        AccountMeta::new_readonly(token::ID,false),
        AccountMeta::new_readonly(token_2022::ID,false),
    ];

    let farm_data = get_farm(svm,&farm_pda);
    assert_eq!(reward_tokens.len(),farm_data.reward_streams_count as usize);

    // Remaining accounts.
    for i in 0..reward_tokens.len() {
        harvest_ixn_accounts.push(AccountMeta::new_readonly(farm_data.reward_streams[i].reward_mint,false));

        let reward_vault = get_associated_token_address_with_program_id(&farm_pda, &farm_data.reward_streams[i].reward_mint, &farm_data.reward_streams[i].reward_mint_program);
        harvest_ixn_accounts.push(AccountMeta::new(reward_vault,false));

        harvest_ixn_accounts.push(AccountMeta::new(reward_tokens[i as usize],false))
    }

    let harvest_ixn = Instruction {
        program_id:RAYDIUM_PROGRAM_ID,
        accounts:harvest_ixn_accounts,
        data:harvest_ixn_data
    };

    let tx = Transaction::new_signed_with_payer(&[harvest_ixn], Some(&staker.pubkey()), &[&staker], svm.latest_blockhash());

    svm.send_transaction(tx)
}

pub struct SetRewardIxn<'a> {
    pub creator:&'a Keypair,
    pub staking_mint:&'a Pubkey,
    pub reward_stream_idx:u8,
    pub updated_reward_stream:RewardStreamArgs
}
pub fn set_reward_ixn(svm:&mut LiteSVM,SetRewardIxn {
    creator,staking_mint,reward_stream_idx,updated_reward_stream
}:SetRewardIxn) -> std::result::Result<TransactionMetadata,FailedTransactionMetadata> {

    let mut hasher = Sha256::new();
    hasher.update("global:set_rewards");
    let hash = hasher.finalize();

    let mut set_reward_ixn_discriminator = [0u8;8];
    set_reward_ixn_discriminator.copy_from_slice(&hash[..8]);
    let mut set_reward_ixn_data = set_reward_ixn_discriminator.to_vec();
    set_reward_ixn_data.extend_from_slice(&reward_stream_idx.to_le_bytes());
    set_reward_ixn_data.extend_from_slice(&updated_reward_stream.open_time.to_le_bytes());
    set_reward_ixn_data.extend_from_slice(&updated_reward_stream.end_time.to_le_bytes());
    set_reward_ixn_data.extend_from_slice(&updated_reward_stream.emission_per_second_x64.to_le_bytes());

    let farm_pda = derive_farm_pda(&staking_mint);
    let farm_data = get_farm(svm,&farm_pda);

    let farm_reward_stream = &farm_data.reward_streams[reward_stream_idx as usize];
    let reward_vault_pda = get_associated_token_address_with_program_id(&farm_pda, &farm_reward_stream.reward_mint, &farm_reward_stream.reward_mint_program);

    let creator_reward_token = get_associated_token_address_with_program_id(&creator.pubkey(), &farm_reward_stream.reward_mint, &farm_reward_stream.reward_mint_program);
    let set_reward_ixn_accounts = vec![
        AccountMeta::new_readonly(creator.pubkey(),true),
        AccountMeta::new_readonly(*staking_mint, false),
        AccountMeta::new(farm_pda,false),
        AccountMeta::new_readonly(farm_reward_stream.reward_mint,false),
        AccountMeta::new(reward_vault_pda,false),
        AccountMeta::new(creator_reward_token,false),
        AccountMeta::new_readonly(farm_reward_stream.reward_mint_program,false),
    ];

    let set_reward_ixn = Instruction {
        program_id: RAYDIUM_PROGRAM_ID,
        accounts: set_reward_ixn_accounts,
        data: set_reward_ixn_data
    };

    let tx = Transaction::new_signed_with_payer(&[set_reward_ixn], Some(&creator.pubkey()), &[&creator], svm.latest_blockhash());

    svm.send_transaction(tx)
}

pub fn time_travel(svm:&mut LiteSVM,jump_by:i64) {
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp = clock.unix_timestamp.checked_add(jump_by).unwrap();
    svm.set_sysvar(&clock);
}

pub struct AddRewardIxn<'a> {
    pub creator:&'a Keypair,
    pub staking_mint:&'a Pubkey,
    pub reward_info:RewardStream<'a>
}
pub fn add_reward(svm:&mut LiteSVM,AddRewardIxn {
    reward_info: RewardStream { reward_mint, open_time, end_time, emission_per_second_x64 },
    creator,
    staking_mint,
}:AddRewardIxn) -> std::result::Result<TransactionMetadata,FailedTransactionMetadata> {
    
    let mut hasher = Sha256::new();
    hasher.update("global:add_reward");
    let hash = hasher.finalize();
    
    let mut add_reward_ixn_discriminator = [0u8;8];
    add_reward_ixn_discriminator.copy_from_slice(&hash[..8]);

    let mut add_reward_ixn_data = add_reward_ixn_discriminator.to_vec();

    add_reward_ixn_data.extend_from_slice(&open_time.to_le_bytes());
    add_reward_ixn_data.extend_from_slice(&end_time.to_le_bytes());
    add_reward_ixn_data.extend_from_slice(&emission_per_second_x64.to_le_bytes());

    let farm_pda = derive_farm_pda(&staking_mint);

    let reward_mint_info = svm.get_account(reward_mint).unwrap();
    let reward_mint_program = reward_mint_info.owner;
    let reward_vault = get_associated_token_address_with_program_id(&farm_pda, &reward_mint,&reward_mint_program );
    let creator_reward_token = get_associated_token_address_with_program_id(&creator.pubkey(), &reward_mint,&reward_mint_program );

    let add_reward_ctx = vec![
        AccountMeta::new(creator.pubkey(),true),
        AccountMeta::new_readonly(*staking_mint, false),
        AccountMeta::new(farm_pda, false),
        AccountMeta::new_readonly(*reward_mint, false),
        AccountMeta::new(reward_vault,false),
        AccountMeta::new(creator_reward_token,false),
        AccountMeta::new_readonly(reward_mint_program,false),
        AccountMeta::new_readonly(system_program::ID,false),
        AccountMeta::new_readonly(associated_token::ID,false),
    ];

    let add_reward_ixn = Instruction {
        program_id:RAYDIUM_PROGRAM_ID,
        accounts:add_reward_ctx,
        data:add_reward_ixn_data,
    };

    let txn = Transaction::new_signed_with_payer(&[add_reward_ixn], Some(&creator.pubkey()), &[&creator], svm.latest_blockhash());
    svm.send_transaction(txn)
    
}



pub struct RestartRewardsIxn<'a> {
    pub creator:&'a Keypair,
    pub staking_mint:&'a Pubkey,
    pub reward_stream_idx:u8,
    pub reward_stream:RewardStreamArgs
}
pub fn restart_rewards(svm:&mut LiteSVM,RestartRewardsIxn {
    creator,staking_mint,reward_stream_idx,reward_stream
}:RestartRewardsIxn) -> std::result::Result<TransactionMetadata,FailedTransactionMetadata> {

    let mut hasher = Sha256::new();
    hasher.update("global:restart_rewards");
    let hash = hasher.finalize();

    let mut restart_rewards_ixn_discriminator = [0u8;8];
    restart_rewards_ixn_discriminator.copy_from_slice(&hash[..8]);
    let mut restart_rewards_ixn_data = restart_rewards_ixn_discriminator.to_vec();

    restart_rewards_ixn_data.extend_from_slice(&reward_stream_idx.to_le_bytes());
    restart_rewards_ixn_data.extend_from_slice(&reward_stream.open_time.to_le_bytes());
    restart_rewards_ixn_data.extend_from_slice(&reward_stream.end_time.to_le_bytes());
    restart_rewards_ixn_data.extend_from_slice(&reward_stream.emission_per_second_x64.to_le_bytes());

    let farm_pda = derive_farm_pda(&staking_mint);
    let farm_data = get_farm(svm,&farm_pda);

    let farm_reward_stream = &farm_data.reward_streams[reward_stream_idx as usize];
    let reward_vault_pda = get_associated_token_address_with_program_id(&farm_pda, &farm_reward_stream.reward_mint, &farm_reward_stream.reward_mint_program);

    let creator_reward_token = get_associated_token_address_with_program_id(&creator.pubkey(), &farm_reward_stream.reward_mint, &farm_reward_stream.reward_mint_program);
    let restart_rewards_ixn = vec![
        AccountMeta::new_readonly(creator.pubkey(),true),
        AccountMeta::new_readonly(*staking_mint, false),
        AccountMeta::new(farm_pda,false),
        AccountMeta::new_readonly(farm_reward_stream.reward_mint,false),
        AccountMeta::new(reward_vault_pda,false),
        AccountMeta::new(creator_reward_token,false),
        AccountMeta::new_readonly(farm_reward_stream.reward_mint_program,false),
    ];

    let restart_rewards_ixn = Instruction {
        program_id: RAYDIUM_PROGRAM_ID,
        accounts: restart_rewards_ixn,
        data: restart_rewards_ixn_data
    };

    let tx = Transaction::new_signed_with_payer(&[restart_rewards_ixn], Some(&creator.pubkey()), &[&creator], svm.latest_blockhash());

    svm.send_transaction(tx)
}
pub struct WithdrawRewardIxn<'a> {
    pub creator:&'a Keypair,
    pub staking_mint:&'a Pubkey,
    pub reward_stream_idx:u8
}
pub fn withdraw_reward(svm:&mut LiteSVM, WithdrawRewardIxn { creator, staking_mint, reward_stream_idx}) -> std::result::Result<TransactionMetadata,FailedTransactionMetadata> {
    let hasher = Sha256::new();
    hasher.update("global:withdraw_reward");
    let hash = hasher.finalize();

    let mut withdraw_reward_ixn_discriminator = [0u8; 8];
    withdraw_reward_ixn_discriminator.copy_from_slice(&hash[..8]);

    let mut withdraw_reward_ixn_data = withdraw_reward_ixn_discriminator.to_vec();
    withdraw_reward_ixn_data.extend_from_slice(&reward_stream_idx.to_le_bytes());

    let farm_pda = derive_farm_pda(staking_mint);
    let farm = get_farm(svm, &farm_pda);
    let farm_reward_stream = &farm.reward_streams[reward_stream_idx as usize];

    let creator_reward_token = get_associated_token_address_with_program_id(&creator.pubkey(), &farm_reward_stream.reward_mint, &farm_reward_stream.reward_mint_program);
    let reward_vault_pda = get_associated_token_address_with_program_id(&farm_pda, &farm_reward_stream.reward_mint, &farm_reward_stream.reward_mint_program);
    
    let withdraw_reward_ixn_ctx = vec![
        AccountMeta::new_readonly(creator.pubkey(),true),
        AccountMeta::new_readonly(*staking_mint,false),
        AccountMeta::new(*farm_pda,false),
        AccountMeta::new_readonly(farm_reward_stream.reward_mint,false),
        AccountMeta::new(creator_reward_token,false),
        AccountMeta::new(reward_vault_pda,false),
        AccountMeta::new_readonly(farm_reward_stream.reward_mint_program,false),
    ];
    
}
