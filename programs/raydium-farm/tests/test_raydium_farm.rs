
use anchor_lang::{prelude::system_program, AccountDeserialize};
use anchor_spl::{associated_token::*,token, token_2022};
use litesvm::*;
use litesvm_token::{
    get_spl_account, spl_token::{native_mint::DECIMALS, state::{Account as TokenAccount,Mint}}, CreateAccount, CreateAssociatedTokenAccount, CreateMint, MintTo, Transfer,TOKEN_ID
};
use raydium_farm::{ utils::*, RewardStreamArgs, RewardStreamStatus};
use sha2::{Sha256, Digest};
use solana_sdk::{
    account::Account, clock::{self, Clock}, message::{AccountMeta, Instruction}, native_token::LAMPORTS_PER_SOL, pubkey::Pubkey, signature::{read_keypair_file, Keypair, Signer}, transaction::Transaction
};

pub mod client;
pub use client::*;



#[test]
pub fn test_raydium_farm() {
    let mut svm =   LiteSVM::new();

    let yash = Keypair::new(); // it's me.
    svm.airdrop(&yash.pubkey(), LAMPORTS_PER_SOL).unwrap();

    let raydium_farm_keypair = read_keypair_file("/home/yashwant/Desktop/web3/raydium-farm/target/deploy/raydium_farm-keypair.json").unwrap();
    let raydium_farm_id = raydium_farm_keypair.pubkey();

    let raydium_farm_bytes = include_bytes!("/home/yashwant/Desktop/web3/raydium-farm/target/deploy/raydium_farm.so");

    svm.add_program(raydium_farm_id, raydium_farm_bytes).unwrap();

    assert!(svm.get_account(&raydium_farm_id).is_some());
    assert!(svm.get_account(&raydium_farm_id).unwrap().executable);

    // Setup
    // Staking mint for the farm we are about to create.
    let staking_mint_authority = Keypair::new();
    let staking_mint = CreateMint::new(&mut svm, &yash).authority(&staking_mint_authority.pubkey()).decimals(0).token_program_id(&token::ID).send().unwrap();
    let staking_mint_program = token::ID;

    // Alice 
    let alice = Keypair::new();
    svm.airdrop(&alice.pubkey(), LAMPORTS_PER_SOL).unwrap();
    let alice_staking_ata = CreateAssociatedTokenAccount::new(&mut svm,&alice,&staking_mint).owner(&alice.pubkey()).send().unwrap();
    MintTo::new(&mut svm,&yash,&staking_mint,&alice_staking_ata,100).owner(&staking_mint_authority).send().unwrap();
    let alice_staking_token: TokenAccount = get_spl_account(&svm, &alice_staking_ata).unwrap();
    assert_eq!(alice_staking_token.amount,100);
    
    // Bob
    let bob = Keypair::new();
    svm.airdrop(&bob.pubkey(), LAMPORTS_PER_SOL).unwrap();
    let bob_staking_ata = CreateAssociatedTokenAccount::new(&mut svm,&bob,&staking_mint).owner(&bob.pubkey()).send().unwrap();
    MintTo::new(&mut svm,&yash,&staking_mint,&bob_staking_ata,100).owner(&staking_mint_authority).send().unwrap();
    let bob_staking_token: TokenAccount = get_spl_account(&svm, &bob_staking_ata).unwrap();
    assert_eq!(bob_staking_token.amount,100);


    let farm_seeds: &[&[u8]] = &[raydium_farm::Farm::STATIC_SEED,&staking_mint.to_bytes()];
    let (farm_pda,farm_bump) = Pubkey::find_program_address(farm_seeds, &raydium_farm_id);

    let reward_0_mint = CreateMint::new(&mut svm, &yash).authority(&yash.pubkey()).decimals(2).token_program_id(&token::ID).send().unwrap();

    let yash_reward_0_token = CreateAssociatedTokenAccount::new(&mut svm,&yash,&reward_0_mint).owner(&yash.pubkey()).send().unwrap();
    MintTo::new(&mut svm,&yash,&reward_0_mint,&yash_reward_0_token, 100000000).owner(&yash).send().unwrap();
    let yash_reward_0_token_account_before:TokenAccount = get_spl_account(&svm, &yash_reward_0_token).unwrap();


    //  Creating a farm with "staking_mint" with 1 reward stream of "reward_mint" (2 decimals),
    //  emission per second = 20111 tokens per 2 seconds = 10055.5 tokens per second. Following the Q64.64 fixed point precision notation, the last 64 bits stores the fractional part of the emission rate
    //  open time = 0 (now),
    //  close time = 3.
    let emission_per_second_x64 = 20111u128.checked_shl(64).unwrap().checked_div(2).unwrap(); 

    let clock:Clock = svm.get_sysvar();
    let open_time = clock.unix_timestamp;
    let mut end_time = clock.unix_timestamp + 3; // This reward stream duration is 3 seconds.

    create_farm(&mut svm,CreateFarmIxn {
        creator:&yash,
        staking_mint:&staking_mint,
        reward_streams:[Some(RewardStream {
            reward_mint:&reward_0_mint,
            open_time,
            end_time,
            emission_per_second_x64,
        }),None,None,None,None]
    }).unwrap();

    // Verifying the farm state.
    let farm_data = get_farm(&svm, &farm_pda);

    assert_eq!(farm_data.authority,yash.pubkey());
    assert_eq!(farm_data.staking_mint,staking_mint);

    let staking_mint_program = svm.get_account(&staking_mint).unwrap().owner;
    assert_eq!(farm_data.staking_mint_program,staking_mint_program);
    assert_eq!(farm_data.staked_amount,0);
    assert_eq!(farm_data.last_updated_time,clock.unix_timestamp);
    assert_eq!(farm_data.reward_streams_count,1);
    assert_eq!(farm_data.bump,farm_bump);

    let farm_reward_stream_0 = &farm_data.reward_streams[0 as usize];
    assert_eq!(farm_reward_stream_0.reward_mint, reward_0_mint);
    assert_eq!(farm_reward_stream_0.open_time, open_time);
    assert_eq!(farm_reward_stream_0.end_time, end_time);
    assert_eq!(farm_reward_stream_0.emission_per_second_x64, emission_per_second_x64);
    verify_updated_farm_status(&svm, &farm_data, 0);
    assert_eq!(farm_reward_stream_0.acc_rewards_per_base_unit_x64,0);
    
    let total_rewards_x64 = emission_per_second_x64.checked_mul(end_time.checked_sub(open_time).unwrap() as u128).unwrap(); // 100.555 * 2^ 64 * 3
    let expected_vault_balance = ceil_div_x64(total_rewards_x64); // ceil((100.555 * 2^ 64 * 3) / 2^64) = 301.67 (301.665).
    assert_eq!(expected_vault_balance, 30167u64);
    assert_eq!(farm_reward_stream_0.rewards_left_x64.checked_shr(64).unwrap() as u64,expected_vault_balance);

    let farm_reward_0_token = get_associated_token_address_with_program_id(&farm_pda, &reward_0_mint,&token::ID);
    let reward_vault_0_token: TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();
    assert_eq!(reward_vault_0_token.amount, expected_vault_balance); // += expected_vault_balance

    let yash_reward_0_token_account: TokenAccount = get_spl_account(&svm, &yash_reward_0_token).unwrap();
    assert_eq!(yash_reward_0_token_account.amount, yash_reward_0_token_account_before.amount - expected_vault_balance); // -= expected_vault_balance

    // Creating Reward tokens for Alice and Bob.
    let mut alice_reward_tokens:Vec<Pubkey> = vec![];
    let mut bob_reward_tokens:Vec<Pubkey> = vec![];

    for i in 0..farm_data.reward_streams_count {
        let alice_reward_token = CreateAssociatedTokenAccount::new(&mut svm,&alice,&farm_data.reward_streams[i as usize].reward_mint).owner(&alice.pubkey()).token_program_id(&farm_data.reward_streams[i as usize].reward_mint_program).send().unwrap();
        alice_reward_tokens.push(alice_reward_token);

        let bob_reward_token = CreateAssociatedTokenAccount::new(&mut svm,&bob,&farm_data.reward_streams[i as usize].reward_mint).owner(&bob.pubkey()).token_program_id(&farm_data.reward_streams[i as usize].reward_mint_program).send().unwrap();
        bob_reward_tokens.push(bob_reward_token);
    }


    let alice_staking_token_before = alice_staking_token;

    let mut alice_staked_amount = 0;
    stake(&mut svm,StakeIxn {
        staker:&alice,staking_mint:&staking_mint,staker_staking_token:&alice_staking_ata,reward_tokens:&alice_reward_tokens,deposit_amount:1
    }).unwrap();
    
    let farm_data_after = get_farm(&svm,&farm_pda);
    assert_eq!(farm_data_after.staked_amount,1); 


    let alice_staking_token_after: TokenAccount = get_spl_account(&svm, &alice_staking_ata).unwrap();
    assert_eq!(alice_staking_token_after.amount,alice_staking_token_before.amount - 1);

    let (alice_ledger_pda,alice_ledger_bump) = derive_user_ledger_pda(&farm_pda,&alice.pubkey());
    let (bob_ledger_pda,bob_ledger_bump) = derive_user_ledger_pda(&farm_pda,&bob.pubkey());

    let alice_ledger_after = get_user_ledger(&svm,&alice_ledger_pda);

    // Verifying the ledger state.
    assert_eq!(alice_ledger_after.user,alice.pubkey());
    assert_eq!(alice_ledger_after.staked_amount,1);
    alice_staked_amount += 1;


    assert_eq!(alice_ledger_after.bump,alice_ledger_bump);

    let alice_reward_info_0 = &alice_ledger_after.reward_infos[0 as usize];
    assert_eq!(alice_reward_info_0.pending_rewards_x64,0);
    assert_eq!(alice_reward_info_0.rewards_debt_x64,0);


    let farm_data_before = farm_data_after;
    let farm_reward_stream_0_before = &farm_data_before.reward_streams[0 as usize];
    let yash_reward_0_token_account_before = yash_reward_0_token_account;
    let reward_vault_0_account_before = reward_vault_0_token;

    let total_rewards_x64 = emission_per_second_x64.checked_mul(4).unwrap(); // 100.555 * 4 = 402.22.
    let expected_transfer_amount = ceil_div_x64(total_rewards_x64.checked_sub(farm_reward_stream_0.rewards_left_x64).unwrap());

    // Extending the reward_stream_0 duration by 1 second with the same emission rate
    end_time += 1;
    set_reward_ixn(&mut svm, SetRewardIxn { creator: &yash, staking_mint: &staking_mint, reward_stream_idx: 0, updated_reward_stream: RewardStreamArgs{
        open_time,
        end_time,
        emission_per_second_x64,
    } }).unwrap();

    assert_eq!(expected_transfer_amount,10055u64); // Prefunded the 0.5 tokens out of 10055.5 to the vault.

    let farm_data_after = get_farm(&svm,&farm_pda);
    let farm_reward_stream_0_after = farm_data_after.reward_streams[0 as usize];
    
    assert_eq!(farm_reward_stream_0_after.rewards_left_x64, farm_reward_stream_0_before.rewards_left_x64.checked_add((expected_transfer_amount as u128).checked_shl(64).unwrap()).unwrap());
    assert_eq!(farm_reward_stream_0_after.rewards_left_x64,40222u128.checked_shl(64).unwrap()); // 10055.5 * 4 = 40222 tokens.
    assert_eq!(farm_reward_stream_0_after.end_time, end_time);
    assert_eq!(farm_reward_stream_0_after.emission_per_second_x64, emission_per_second_x64);
    
    let yash_reward_0_token_account_after: TokenAccount = get_spl_account(&svm, &yash_reward_0_token).unwrap();
    assert_eq!(yash_reward_0_token_account_after.amount, yash_reward_0_token_account_before.amount.checked_sub(expected_transfer_amount).unwrap());

    let reward_vault_0_account_after: TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();
    assert_eq!(reward_vault_0_account_after.amount, reward_vault_0_account_before.amount.checked_add(expected_transfer_amount).unwrap());


    //  Current state t = 0, Yash created a farm with "staking_mint" with one reward stream of "reward_mint" (2 decimals), Extended the end_time 
    //  with "set_rewards" ixn and Alice staked 100 tokens to the farm. 
    //  emission_per_second_x64 for the 1st reward stream = 10055.5 * 2^64 (2 decimals reward token),
    //  acc_rewards_per_base_unit_x64 for the 1st reward stream = 0 * 2^64.
    //  farm.staked_amount = 100.
    //  rewards_left_x64[0] = 40222 * 2^64 . (reward_vault[0] * 2^64 >= rewards_left_x64 )
    //  Alce's staked_amount = 100.
    //  Alice's pending_rewards_x64[0] of the 1st reward stream = 0 * 2^64 (Nothing is owed by the farm as the deposition happened at this exact instant),
    //  Alice's rewards_debt_x64[0] of the 1st reward stream = 0 * 2^64 (No rewards is missed or collected)
    
    let farm_reward_stream_0_before = farm_reward_stream_0_after;
    let farm_data_before = farm_data_after;
    let alice_ledger_data_before = alice_ledger_after;
    let alice_ledger_reward_stream_0_before = alice_ledger_data_before.reward_infos[0 as usize];

    let alice_reward_0_token_before:TokenAccount = get_spl_account(&svm, &alice_reward_tokens[0 as usize]).unwrap();

    time_travel(&mut svm, 1); 
    let clock = svm.get_sysvar::<Clock>();
    assert_eq!(clock.unix_timestamp,1);

    // t = 1, Alice harvests.
    harvest(&mut svm,HarvestIxn {
        staker:&alice,
        staking_mint:&staking_mint,
        reward_tokens:&alice_reward_tokens
    }).unwrap();

    // 6 Exhaustive critical checks:
    //  new_emission_x64[0] = (emission_per_second_x64[0] * duration(now,farm_before.last_update_time)) = 10055.5 * 2^64 * 1
    //  1) new_acc_rewards_per_base_unit_x64[0]= acc_rewards_per_base_unit_x64[0] + new_emission_x64[0] / total_staked_amount = 0 * 2^64 + ((10055.5 * 2^64) / 1) = 10055.5 * 2^64.
    //  2) new rewards_left_x64[0] = rewards_left_x64[0] - new_emission_x64[0] = 40222 * 2^64 - 10055.5 * 2^64 = 30166.5 * 2^64
    //  new_alice_rewards = new_acc_rewards_per_base_unit_x64[0] * alice.staked_amount_before - rewards_debt_x64_before[0] = 10055.5 * 2^64 * 1 - 0 = 10055.5 * 2^64 (100% of the emitted tokens of this second is owed to Alice as She is the only staker).
    //  3) Alice's harvested rewards = Alice's pending_rewards_x64[0] + new_alice_rewards = 0 + 10055.5 * 2^64 = 10055.5 * 2^64 out of which >>64 = 10055 (note: if I do *_x64 >>64, I will only retain the integer part, i.e, 1.5 >>64 = 1u64) is transfered to the token account.
    //  4) While the amount to be transferred - Amount transferred = 0.5 (Lesser than denomination that can be transfered) is stored in Alice's pending_rewards_x64[0] = 0.5 * 2^64 (The last 64 bits holds the fractional value, follows Q64.64 fixed point precision notation).
    //  5) new_reward_vault_balance[0] = rewards_vault_balance[0] - Amount transfered to staker(s) = 40222 - 10055 = 30167 (reward_vault[0] * 2^64 >= rewards_left_x64) but out of which 0.5 is owed to Alice and locked, If Alice held her stake for one more second she will receive it. Will see this in next test. 
    //  6) Alice's rewards_debt_x64[0] = Alice's rewards_debts_x64_old[0] + new_alice_rewards = 0 + 10055.5 * 2^64 = 10055.5 * 2^64 (This rewards_debt_x64 represents the missed or collected rewards of the respective stream)

    let farm_data_after = get_farm(&svm, &farm_pda);
    let farm_reward_stream_0_after = farm_data_after.reward_streams[0];

    let alice_ledger_data_after = get_user_ledger(&svm, &alice_ledger_pda);
    let alice_ledger_reward_stream_0_after = alice_ledger_data_after.reward_infos[0];
    let alice_reward_0_token_after:TokenAccount = get_spl_account(&svm, &alice_reward_tokens[0 as usize]).unwrap(); 

    let duration = clock.unix_timestamp.checked_sub(farm_data_before.last_updated_time.max(farm_reward_stream_0_before.open_time)).unwrap() as u128; 
    assert_eq!(duration,1);

    let new_emissions_x64 = duration.checked_mul(emission_per_second_x64).unwrap();
    assert_eq!(new_emissions_x64,100555u128.checked_shl(64).unwrap().checked_div(10).unwrap());

    let new_rewards_per_base_unit_x64 = new_emissions_x64.checked_div(1).unwrap();
    assert_eq!(new_rewards_per_base_unit_x64,100555u128.checked_shl(64).unwrap().checked_div(10).unwrap());

    let expected_acc_rewards_per_base_unit_x64 = 0u128.checked_add(new_rewards_per_base_unit_x64).unwrap();
    assert_eq!(farm_reward_stream_0_after.acc_rewards_per_base_unit_x64,expected_acc_rewards_per_base_unit_x64);    // - (1)
    let farm_acc_rewards_per_base_unit_x64 = expected_acc_rewards_per_base_unit_x64;
    
    let expected_farm_rewards_left_x64 = 40222u128.checked_shl(64).unwrap().checked_sub(new_emissions_x64).unwrap();
    assert_eq!(expected_farm_rewards_left_x64,301665u128.checked_shl(64).unwrap().checked_div(10).unwrap());
    assert_eq!(farm_reward_stream_0_after.rewards_left_x64,expected_farm_rewards_left_x64);    // - (2)

    let new_alice_rewards = farm_acc_rewards_per_base_unit_x64.checked_mul(alice_staked_amount).unwrap().checked_sub(0u128).unwrap();
    let total_unclaimed_alice_rewards = 0u128.checked_add(new_alice_rewards).unwrap();
    let transfered_amount  = total_unclaimed_alice_rewards.checked_shr(64).unwrap() as u64;
    assert_eq!(10055,transfered_amount);
    assert_eq!(alice_reward_0_token_after.amount.checked_sub(alice_reward_0_token_before.amount).unwrap(),transfered_amount);    // - (3)

    let expected_alice_reward_0_rewards_debt_x64 = 0u128.checked_add(new_alice_rewards).unwrap();
    assert_eq!(expected_alice_reward_0_rewards_debt_x64,100555u128.checked_shl(64).unwrap().checked_div(10).unwrap());
    assert_eq!(alice_ledger_reward_stream_0_after.rewards_debt_x64,expected_alice_reward_0_rewards_debt_x64);    // - (5)

    let expected_alice_reward_0_pending_rewards_x64 = total_unclaimed_alice_rewards.checked_sub((transfered_amount as u128).checked_shl(64).unwrap()).unwrap();
    assert_eq!(expected_alice_reward_0_pending_rewards_x64,5u128.checked_shl(64).unwrap().checked_div(10).unwrap());
    assert_eq!(alice_ledger_reward_stream_0_after.pending_rewards_x64,expected_alice_reward_0_pending_rewards_x64);    // - (5)

    time_travel(&mut svm,1);
    let clock = svm.get_sysvar::<Clock>();
    assert_eq!(clock.unix_timestamp,2);

    let farm_before = get_farm(&svm,&farm_pda);
    let farm_reward_stream_0_before = farm_before.reward_streams[0];
    let farm_reward_0_token_before:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();

    let alice_ledger_before = get_user_ledger(&svm, &alice_ledger_pda);
    let alice_ledger_reward_stream_0_before = &alice_ledger_before.reward_infos[0];
    let alice_reward_0_token_before:TokenAccount = get_spl_account(&svm, &alice_reward_tokens[0]).unwrap();

    svm.expire_blockhash();

    harvest(&mut svm,HarvestIxn {
        staker:&alice,
        staking_mint:&staking_mint,
        reward_tokens:&alice_reward_tokens
    }).unwrap();

    // t = 2
    // 6 Exhaustive critical checks:
    //  new_emission_x64[0] = (emission_per_second_x64[0] * duration(now,farm_before.last_update_time)) = 10055.5 * 2^64 * 1
    //  1) new_acc_rewards_per_base_unit_x64[0]= acc_rewards_per_base_unit_x64[0] + new_emission_x64[0] / total_staked_amount = 10055.5 * 2^64 + (10055.5 * 2^64) / 1 = 20111 * 2^64.
    //  2) new rewards_left_x64[0] = rewards_left_x64[0] - new_emission_x64[0] = 30166.5 * 2^64 - 10055.5 * 2^64 = 20111 * 2^64 
    //  new_alice_rewards = new_acc_rewards_per_base_unit_x64[0] * alice.staked_amount_before - rewards_debt_x64_before[0] = 20111 ^ 2^64 * 1 - 10055.5 * 2^64  = 10055.5 * 2^64 (100% of the emitted tokens of this second is owed to Alice as She is the only staker).
    //  3) Alice's harvested rewards = Alice's pending_rewards_x64[0] + new_alice_rewards = 0.5 * 2^64 + 10055.5 * 2^64 = 10056 * 2^64 >>64 = 10056 is transfered to the token account.
    //  4) Alice's pending_rewards_x64[0] = 0 * 2^64. (IMPORTANT: Alice was rewarded 10055 at t = 1 and 10056 at t = 2 making the total 20111 and since Alice is the only staker and emission rate is set to "20111 tokens per 2 seconds (2 decimal reward mint)", She got all the reward but the interesting thing is how the reward was processed, at t = 1, 10055 transferred + 0.5 pending and at t = 2, 10056 added the pending and then transferred)
    //  5) new_reward_vault_balance[0] = rewards_vault_balance[0] - Amount harvested by staker(s) = 30167 - 10056 = 20111.
    //  6) Alice's rewards_debt_x64[0] = Alice's rewards_debts_x64_old[0] + new_alice_rewards = 10055.5 * 2^64 + 10055.5 * 2^64 = 20111 * 2^64.

    let farm_after = get_farm(&svm,&farm_pda);
    let farm_reward_stream_0_after = farm_after.reward_streams[0];
    let farm_reward_0_token_after:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();

    let alice_ledger_after = get_user_ledger(&svm, &alice_ledger_pda);
    let alice_ledger_reward_stream_0_after = &alice_ledger_after.reward_infos[0];
    let alice_reward_0_token_after:TokenAccount = get_spl_account(&svm, &alice_reward_tokens[0]).unwrap();
    
    assert_eq!(farm_after.last_updated_time,2);

    let duration = clock.unix_timestamp.checked_sub(farm_before.last_updated_time.max(farm_reward_stream_0_before.open_time)).unwrap() as u128; 
    assert_eq!(duration,1);

    let new_emissions_x64 = duration.checked_mul(emission_per_second_x64).unwrap();
    assert_eq!(new_emissions_x64,100555u128.checked_shl(64).unwrap().checked_div(10).unwrap());

    let new_rewards_per_base_unit_x64 = new_emissions_x64.checked_div(1).unwrap();
    assert_eq!(new_rewards_per_base_unit_x64,100555u128.checked_shl(64).unwrap().checked_div(10).unwrap());

    let expected_acc_rewards_per_base_unit_x64 = farm_before.reward_streams[0].acc_rewards_per_base_unit_x64.checked_add(new_rewards_per_base_unit_x64).unwrap();
    assert_eq!(expected_acc_rewards_per_base_unit_x64,20111u128.checked_shl(64).unwrap());
    assert_eq!(farm_reward_stream_0_after.acc_rewards_per_base_unit_x64,expected_acc_rewards_per_base_unit_x64);    // - (1)
    
    let expected_farm_rewards_left_x64 = farm_before.reward_streams[0].rewards_left_x64.checked_sub(new_emissions_x64).unwrap();
    assert_eq!(expected_farm_rewards_left_x64,20111u128.checked_shl(64).unwrap());
    assert_eq!(farm_reward_stream_0_after.rewards_left_x64,expected_farm_rewards_left_x64);    // - (2)

    let new_alice_rewards_x64 = farm_reward_stream_0_after.acc_rewards_per_base_unit_x64.checked_mul(alice_staked_amount).unwrap().checked_sub(alice_ledger_reward_stream_0_before.rewards_debt_x64).unwrap();
    assert_eq!(new_alice_rewards_x64,100555u128.checked_shl(64).unwrap().checked_div(10).unwrap());

    let total_unclaimed_alice_rewards = alice_ledger_before.reward_infos[0].pending_rewards_x64.checked_add(new_alice_rewards_x64).unwrap();
    let transfered_amount  = total_unclaimed_alice_rewards.checked_shr(64).unwrap() as u64;
    assert_eq!(10056,transfered_amount);
    assert_eq!(alice_reward_0_token_after.amount.checked_sub(alice_reward_0_token_before.amount).unwrap(),transfered_amount);    // - (3)
    assert_eq!(farm_reward_0_token_before.amount.checked_sub(farm_reward_0_token_after.amount).unwrap(),transfered_amount);

    let expected_alice_reward_0_rewards_debt_x64 = alice_ledger_before.reward_infos[0].rewards_debt_x64.checked_add(new_alice_rewards_x64).unwrap();
    assert_eq!(expected_alice_reward_0_rewards_debt_x64,20111u128.checked_shl(64).unwrap());
    assert_eq!(alice_ledger_reward_stream_0_after.rewards_debt_x64,expected_alice_reward_0_rewards_debt_x64);    // - (4)

    let expected_alice_reward_0_pending_rewards_x64 = total_unclaimed_alice_rewards.checked_sub((transfered_amount as u128).checked_shl(64).unwrap()).unwrap();
    assert_eq!(expected_alice_reward_0_pending_rewards_x64,0);
    assert_eq!(alice_ledger_reward_stream_0_after.pending_rewards_x64,expected_alice_reward_0_pending_rewards_x64);    // - (5)

    // Also at t = 2, Alice stakes more 2 token, Bob stakes 1 token. No change in the farm's reward_streams as last_updated_time == current time, therefore no change in the Alice's user_ledger's reward_infos
    // Alice's user_ledger -> 
    //  new staked_amount = 1 + 2 = 3
    //  new_rewards_debt_x64[0] = rewards_debt_x64[0] + new_staked_amount * new_acc_rewards_per_base_unit_x64[0] = 20111 * 2^64 + 2 * 20111 * 2^64 = 60333 * 2^64
    
    // Bob's user_ledger -> 
    //  new staked_amount = 1
    //  new_rewards_debt_x64[0] = rewards_debt_x64[0] + new_staked_amount * new_acc_rewards_per_base_unit_x64[0] = 0 * 2^64 + 1 * 20111 * 2^64 = 20111 * 2^64

    // farm.staked_amount += 3 (2 + 1) = 4

    let farm_staking_token = get_associated_token_address_with_program_id(&farm_pda, &staking_mint, &staking_mint_program);

    let farm_before = get_farm(&svm,&farm_pda);
    let farm_reward_stream_0_before = farm_before.reward_streams[0];
    let farm_staking_token_account_before:TokenAccount = get_spl_account(&svm, &farm_staking_token).unwrap();

    let alice_ledger_before = get_user_ledger(&svm, &alice_ledger_pda);
    let alice_ledger_reward_stream_0_before = &alice_ledger_before.reward_infos[0];
    let alice_reward_0_token_before:TokenAccount = get_spl_account(&svm, &alice_reward_tokens[0]).unwrap();
    let bob_reward_0_token_before:TokenAccount = get_spl_account(&svm, &bob_reward_tokens[0]).unwrap();

    let alice_stake_amount = 2;
    let bob_stake_amount = 1;
    stake(&mut svm, StakeIxn { staker: &alice, staking_mint: &staking_mint, staker_staking_token: &alice_staking_ata, reward_tokens: &alice_reward_tokens, deposit_amount: alice_stake_amount }).unwrap();
    stake(&mut svm, StakeIxn { staker: &bob, staking_mint: &staking_mint, staker_staking_token: &bob_staking_ata, reward_tokens: &bob_reward_tokens, deposit_amount: bob_stake_amount }).unwrap();

    let farm_after = get_farm(&svm,&farm_pda);
    let farm_reward_stream_0_after = farm_after.reward_streams[0];
    let farm_staking_token_account_after:TokenAccount = get_spl_account(&svm, &farm_staking_token).unwrap();

    let alice_ledger_after = get_user_ledger(&svm, &alice_ledger_pda);
    let alice_ledger_reward_stream_0_after = &alice_ledger_after.reward_infos[0];
    let alice_reward_0_token_after:TokenAccount = get_spl_account(&svm, &alice_reward_tokens[0]).unwrap();

    let bob_ledger_after = get_user_ledger(&svm, &bob_ledger_pda);
    let bob_ledger_reward_stream_0_after = &bob_ledger_after.reward_infos[0];
    let bob_reward_0_token_after:TokenAccount = get_spl_account(&svm, &bob_reward_tokens[0]).unwrap();

    
    assert_eq!(farm_after.last_updated_time,2);
    assert_eq!(farm_after.staked_amount,farm_before.staked_amount.checked_add(alice_stake_amount + bob_stake_amount).unwrap());
    assert_eq!(farm_after.staked_amount,4);
    assert_eq!(farm_staking_token_account_after.amount,farm_staking_token_account_before.amount.checked_add(3).unwrap());


    assert_eq!(alice_ledger_after.staked_amount,alice_ledger_before.staked_amount.checked_add(alice_stake_amount).unwrap());
    assert_eq!(alice_ledger_after.staked_amount,3);

    assert_eq!(bob_ledger_after.staked_amount,bob_stake_amount);

    assert_eq!(farm_reward_stream_0_after.acc_rewards_per_base_unit_x64,farm_reward_stream_0_before.acc_rewards_per_base_unit_x64);
    assert_eq!(farm_reward_stream_0_after.rewards_left_x64,farm_reward_stream_0_before.rewards_left_x64);

    assert_eq!(alice_ledger_reward_stream_0_after.pending_rewards_x64,alice_ledger_reward_stream_0_before.pending_rewards_x64);

    assert_eq!(alice_ledger_reward_stream_0_after.rewards_debt_x64,alice_ledger_reward_stream_0_before.rewards_debt_x64.checked_add(farm_reward_stream_0_after.acc_rewards_per_base_unit_x64.checked_mul(alice_stake_amount.into()).unwrap()).unwrap());
    assert_eq!(alice_ledger_reward_stream_0_after.rewards_debt_x64,60333u128.checked_shl(64).unwrap());

    assert_eq!(bob_ledger_reward_stream_0_after.rewards_debt_x64,0u128.checked_add(farm_reward_stream_0_after.acc_rewards_per_base_unit_x64.checked_mul(bob_stake_amount.into()).unwrap()).unwrap());
    assert_eq!(bob_ledger_reward_stream_0_after.rewards_debt_x64,20111u128.checked_shl(64).unwrap());


    time_travel(&mut svm,1);
    let clock = svm.get_sysvar::<Clock>();
    assert_eq!(clock.unix_timestamp,3);

    // t = 3, Alice withdraws all her staked assets and Bob harvests.
    // Exhaustive critical checks:
    //  new_emission_x64[0] = (emission_per_second_x64[0] * duration(now,farm_before.last_update_time)) = 10055.5 * 2^64 * 1. (NOTE: If staked amount is 0, new emission is 0)
    //  1) new_acc_rewards_per_base_unit_x64[0]= acc_rewards_per_base_unit_x64[0] + new_emission_x64[0] / total_staked_amount = 20111 * 2^64 + (10055.5 * 2^64) / 4 = 22624.875 * 2^64.
    //  2) new rewards_left_x64[0] = rewards_left_x64[0] - new_emission_x64[0] = 20111 * 2^64 - 10055.5 * 2^64 = 10055.5 * 2^64 (left for last emission in the next second)
    //  3) farm.staked_amount -= 3, staking_mint_vault -= 3

    //  new_alice_rewards = new_acc_rewards_per_base_unit_x64[0] * alice.staked_amount_before - alice.rewards_debt_x64_before[0] = 22624.875 * 2^64 * 3 - 60333 * 2^64  = 7541.625 * 2^64
    //  3) Alice's harvested rewards = Alice's pending_rewards_x64[0] + new_alice_rewards = 0 * 2^64 + 7541.625 * 2^64 = 7541.625 * 2^64 >>64 = 7541 * 2^64 is transfered to the token account.
    //  4) Alice's pending_rewards_x64[0] = 0.625 * 2^64. 
    //  6) Alice's rewards_debt_x64[0] = Alice's rewards_debts_x64_old[0] + new_alice_rewards - withdraw_amount * new_acc_rewards_per_base_unit_x64 = 60333 * 2^64 + 7541.625 * 2^64 - 3 * 22624.875 * 2^64 = 0 * 2^64.

    //  new_bob_rewards = new_acc_rewards_per_base_unit_x64[0] * bob.staked_amount_before - rewards_debt_x64_before[0] = 22624.875 * 2^64 * 1 - 20111 * 2^64  = 2513.875 * 2^64
    //  7) Bob's harvested rewards = Bob's pending_rewards_x64[0] + new_bob_rewards = 0 * 2^64 + 2513.875 * 2^64 = 2513.875 * 2^64 >>64 = 2513 is transfered to the token account.
    //  8) Bob's pending_rewards_x64[0] = 0.875 * 2^64. 
    //  9) Bob's rewards_debt_x64[0] = Bob's rewards_debts_x64_old[0] + new_bob_rewards = 20111 * 2^64 + 2513.875 * 2^64 = 22624.875 * 2^64.
    
    // Reward emitted was divided between the stakers(2) Alice and Bob, 7541.625 * 2^64 and 2513.875 * 2^64 respectively. total = 10055.5 * 2^64 == emission_per_second_x64[0]. 
    //  10) new_reward_vault_balance[0] = rewards_vault_balance[0] - Amount harvested by staker(s) = 20111 - (7541 + 2513)  = 10057 out of which 0.625 + 0.875 = 1.5 is owed to Alice and Bob respectively, 10057 - 1.5 = 10055.5 is left for next emission. (NOTE: There was no truncated division (denominator was a divisor) when calculating new_acc_rewards_per_base_unit_x64 otherwise we would have been left with more and rewarded or owed less to the stakers)
    

    let farm_before = get_farm(&svm,&farm_pda);
    let farm_reward_stream_0_before = farm_before.reward_streams[0];
    let farm_staking_token_account_before:TokenAccount = get_spl_account(&svm, &farm_staking_token).unwrap();
    let farm_reward_0_token_account_before:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();


    let alice_ledger_before = get_user_ledger(&svm, &alice_ledger_pda);
    let alice_ledger_reward_stream_0_before = &alice_ledger_before.reward_infos[0];
    let alice_reward_0_token_before:TokenAccount = get_spl_account(&svm, &alice_reward_tokens[0]).unwrap();


    unstake(&mut svm,UnstakeIxn { staker: &alice, staking_mint: &staking_mint, staker_staking_token: &alice_staking_ata, reward_tokens: &alice_reward_tokens, withdraw_amount: alice_ledger_before.staked_amount }).unwrap();

    let farm_after = get_farm(&svm,&farm_pda);
    let farm_reward_stream_0_after = farm_after.reward_streams[0];
    let farm_staking_token_account_after:TokenAccount = get_spl_account(&svm, &farm_staking_token).unwrap();
    let farm_reward_0_token_account_after:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();


    let alice_ledger_after = get_user_ledger(&svm, &alice_ledger_pda);
    let alice_ledger_reward_stream_0_after = &alice_ledger_after.reward_infos[0];
    let alice_reward_0_token_after:TokenAccount = get_spl_account(&svm, &alice_reward_tokens[0]).unwrap();

    assert_eq!(farm_after.last_updated_time,3);
    assert_eq!(farm_staking_token_account_after.amount,farm_staking_token_account_before.amount.checked_sub(alice_ledger_before.staked_amount).unwrap());
    assert_eq!(farm_after.staked_amount,1);
    
    let duration = clock.unix_timestamp.checked_sub(farm_before.last_updated_time.max(farm_reward_stream_0_before.open_time)).unwrap() as u128; 
    assert_eq!(duration,1);

    let new_emissions_x64 = duration.checked_mul(emission_per_second_x64).unwrap();
    assert_eq!(new_emissions_x64,100555u128.checked_shl(64).unwrap().checked_div(10).unwrap());

    let new_rewards_per_base_unit_x64 = new_emissions_x64.checked_div(farm_before.staked_amount.into()).unwrap();
    assert_eq!(new_rewards_per_base_unit_x64,2513875u128.checked_shl(64).unwrap().checked_div(1000).unwrap());

    let expected_acc_rewards_per_base_unit_x64 = farm_reward_stream_0_before.acc_rewards_per_base_unit_x64.checked_add(new_rewards_per_base_unit_x64).unwrap();
    assert_eq!(expected_acc_rewards_per_base_unit_x64,22624875u128.checked_shl(64).unwrap().checked_div(1000).unwrap());
    assert_eq!(farm_reward_stream_0_after.acc_rewards_per_base_unit_x64,expected_acc_rewards_per_base_unit_x64);    // - (1)
    
    let expected_farm_rewards_left_x64 = farm_reward_stream_0_before.rewards_left_x64.checked_sub(new_emissions_x64).unwrap();
    assert_eq!(expected_farm_rewards_left_x64,100555u128.checked_shl(64).unwrap().checked_div(10).unwrap());
    assert_eq!(farm_reward_stream_0_after.rewards_left_x64,expected_farm_rewards_left_x64);    // - (2)

    let new_alice_rewards_x64 = farm_reward_stream_0_after.acc_rewards_per_base_unit_x64.checked_mul(alice_ledger_before.staked_amount.into()).unwrap().checked_sub(alice_ledger_reward_stream_0_before.rewards_debt_x64).unwrap();
    assert_eq!(new_alice_rewards_x64,7541625u128.checked_shl(64).unwrap().checked_div(1000).unwrap());

    let total_unclaimed_alice_rewards = alice_ledger_reward_stream_0_before.pending_rewards_x64.checked_add(new_alice_rewards_x64).unwrap();
    let transfered_amount  = total_unclaimed_alice_rewards.checked_shr(64).unwrap() as u64;
    assert_eq!(7541,transfered_amount);
    assert_eq!(alice_reward_0_token_after.amount.checked_sub(alice_reward_0_token_before.amount).unwrap(),transfered_amount);    // - (3)
    assert_eq!(farm_reward_0_token_account_before.amount.checked_sub(farm_reward_0_token_account_after.amount).unwrap(),transfered_amount);

    let expected_alice_reward_0_rewards_debt_x64 = alice_ledger_reward_stream_0_before.rewards_debt_x64.checked_add(new_alice_rewards_x64).unwrap().checked_sub(farm_reward_stream_0_after.acc_rewards_per_base_unit_x64.checked_mul(alice_ledger_before.staked_amount.into()).unwrap()).unwrap();
    assert_eq!(expected_alice_reward_0_rewards_debt_x64,0);
    assert_eq!(alice_ledger_reward_stream_0_after.rewards_debt_x64,expected_alice_reward_0_rewards_debt_x64);    // - (4)
    
    let expected_alice_reward_0_pending_rewards_x64 = total_unclaimed_alice_rewards.checked_sub((transfered_amount as u128).checked_shl(64).unwrap()).unwrap();
    assert_eq!(expected_alice_reward_0_pending_rewards_x64,625u128.checked_shl(64).unwrap().checked_div(1000).unwrap());
    assert_eq!(alice_ledger_reward_stream_0_after.pending_rewards_x64,expected_alice_reward_0_pending_rewards_x64);    // - (5)

    let farm_reward_0_token_account_before:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();
    let bob_ledger_before = get_user_ledger(&svm, &bob_ledger_pda);
    let bob_ledger_reward_stream_0_before = &bob_ledger_before.reward_infos[0];
    let bob_reward_0_token_before:TokenAccount = get_spl_account(&svm, &bob_reward_tokens[0]).unwrap();

    harvest(&mut svm,HarvestIxn {
        staker:&bob,
        staking_mint:&staking_mint,
        reward_tokens:&bob_reward_tokens
    }).unwrap();

    let farm_reward_0_token_account_after:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();
    let bob_ledger_after = get_user_ledger(&svm, &bob_ledger_pda);
    let bob_ledger_reward_stream_0_after = &bob_ledger_after.reward_infos[0];
    let bob_reward_0_token_after:TokenAccount = get_spl_account(&svm, &bob_reward_tokens[0]).unwrap();

    let new_bob_rewards_x64 = farm_reward_stream_0_after.acc_rewards_per_base_unit_x64.checked_mul(bob_ledger_before.staked_amount.into()).unwrap().checked_sub(bob_ledger_reward_stream_0_before.rewards_debt_x64).unwrap();
    assert_eq!(new_bob_rewards_x64,2513875u128.checked_shl(64).unwrap().checked_div(1000).unwrap());

    let total_unclaimed_bob_rewards = bob_ledger_reward_stream_0_before.pending_rewards_x64.checked_add(new_bob_rewards_x64).unwrap();
    let transfered_amount  = total_unclaimed_bob_rewards.checked_shr(64).unwrap() as u64;
    assert_eq!(2513,transfered_amount);
    assert_eq!(bob_reward_0_token_after.amount.checked_sub(bob_reward_0_token_before.amount).unwrap(),transfered_amount);    // - (3)
    assert_eq!(farm_reward_0_token_account_before.amount.checked_sub(farm_reward_0_token_account_after.amount).unwrap(),transfered_amount);
    assert_eq!(farm_reward_0_token_account_after.amount,10057);


    let expected_bob_reward_0_pending_rewards_x64 = total_unclaimed_bob_rewards.checked_sub((transfered_amount as u128).checked_shl(64).unwrap()).unwrap();
    assert_eq!(expected_bob_reward_0_pending_rewards_x64,875u128.checked_shl(64).unwrap().checked_div(1000).unwrap());
    assert_eq!(bob_ledger_reward_stream_0_after.pending_rewards_x64,expected_bob_reward_0_pending_rewards_x64);    // - (5)

    let expected_bob_reward_0_rewards_debt_x64 = bob_ledger_reward_stream_0_before.rewards_debt_x64.checked_add(new_bob_rewards_x64).unwrap();
    assert_eq!(expected_bob_reward_0_rewards_debt_x64,22624875u128.checked_shl(64).unwrap().checked_div(1000).unwrap());
    assert_eq!(bob_ledger_reward_stream_0_after.rewards_debt_x64,expected_bob_reward_0_rewards_debt_x64);    // - (4)
    
    // Also at t = 3, Adding a new reward stream with reward mint of 6 decimals open time to be t = 5 and close time to be t = 8,and this time simple emission per second = 1.000000 token per second (1 token/sec) 
    //  1) acc_rewards_per_base_unit_x64[1] = 0 * 2^64.
    //  2) rewards_left_x64[1] = 3000000 * 2^64.
    //  3) emission_per_second_x64[1] = 100000 * 2^64.
    //  4) status[1] = RewardStreamStatus::Unused.
    //  5) open_time[1] = 5,
    //  6) close_time[1] = 8,
    //  3) reward vault of 2nd stream balance = 3000000 tokens.
    
    let reward_1_mint = CreateMint::new(&mut svm,&yash).authority(&yash.pubkey()).decimals(6).token_program_id(&token::ID).send().unwrap();
    let yash_reward_1_token = CreateAssociatedTokenAccount::new(&mut svm,&yash,&reward_1_mint).owner(&yash.pubkey()).token_program_id(&token::ID).send().unwrap();
    let alice_reward_1_token = CreateAssociatedTokenAccount::new(&mut svm,&alice,&reward_1_mint).owner(&alice.pubkey()).token_program_id(&token::ID).send().unwrap();
    alice_reward_tokens.push(alice_reward_1_token);
    let bob_reward_1_token = CreateAssociatedTokenAccount::new(&mut svm,&bob,&reward_1_mint).owner(&bob.pubkey()).token_program_id(&token::ID).send().unwrap();
    bob_reward_tokens.push(bob_reward_1_token);


    MintTo::new(&mut svm, &yash, &reward_1_mint, &yash_reward_1_token, 7500000).send().unwrap(); // Minting just enough.
    
    let open_time = clock.unix_timestamp + 2;
    let end_time = clock.unix_timestamp + 2 + 3; 

    let emission_per_second_x64 = 1000000u128.checked_shl(64).unwrap();

    add_reward(&mut svm, AddRewardIxn {
        creator:&yash,
        staking_mint:&staking_mint,
        reward_info:RewardStream { reward_mint: &reward_1_mint, open_time, end_time, emission_per_second_x64}
    }).unwrap();
    
    let farm = get_farm(&svm,&farm_pda);
    assert_eq!(farm.reward_streams_count,2);
    
    let farm_reward_1_token = get_associated_token_address_with_program_id(&farm_pda, &reward_1_mint,&token::ID);
    let reward_vault_1_token_account: TokenAccount = get_spl_account(&svm, &farm_reward_1_token).unwrap();
    assert_eq!(reward_vault_1_token_account.amount,3000000);

    let farm_reward_1_stream = &farm.reward_streams[1];
    
    assert_eq!(farm_reward_1_stream.status,RewardStreamStatus::Unused);
    assert_eq!(farm_reward_1_stream.open_time,open_time);
    assert_eq!(farm_reward_1_stream.end_time,end_time);
    assert_eq!(farm_reward_1_stream.emission_per_second_x64,emission_per_second_x64);

    assert_eq!(farm_reward_1_stream.acc_rewards_per_base_unit_x64,0);
    assert_eq!(farm_reward_1_stream.rewards_left_x64,3000000u128.checked_shl(64).unwrap());
    
    let farm_reward_1_token: Pubkey = get_associated_token_address_with_program_id(&farm_pda, &reward_1_mint,&token::ID );
    let farm_reward_1_token_account = get_spl_account::<TokenAccount>(&svm,&farm_reward_1_token).unwrap();
    assert_eq!(farm_reward_1_token_account.amount,3000000);

    time_travel(&mut svm,2);
    let clock = svm.get_sysvar::<Clock>();
    assert_eq!(clock.unix_timestamp,5);

    // At t = 5, Bob harvests. Also restarting the 1st reward stream later which ended a second ago (when t = 4).
    //  From the 1st reward stream -> Bob is rewarded for 1 second though 2 seconds have been passed since last harvest but the reward stream itself was closed a second ago.
    //  From the 2nd reward stream -> Nothing. It is opened just now. Status = Running.
    //  new_emission_x64[0] = (emission_per_second_x64[0] * duration(end_time,min(end_time,last_updated_time))) = 10055.5 * 2^64 * 4 - min(4,3) = 10055.5 * 2^64 * 1
    //  1) new_acc_rewards_per_base_unit_x64[0]= acc_rewards_per_base_unit_x64[0] + new_emission_x64[0] / total_staked_amount = 22624.875 + (10055.5 * 2^64) / 1 = 32680.375 * 2^64.
    //  new_bob_rewards = new_acc_rewards_per_base_unit_x64[0] * bob.staked_amount_before - rewards_debt_x64_before[0] = 32680.375 * 2^64 * 1 - 22624.875 * 2^64  = 10055.5 * 2^64 (100% of the emitted tokens of this second is owed to Bob as he is the only staker).
    //  2) Bob's harvested rewards = Bob's pending_rewards_x64[0] + new_bob_rewards = 0.875 * 2^64 + 10055.5 * 2^64 = 10056.375 * 2^64 >>64 = 10056 is transfered to the token account.
    //  3) Bob's pending_rewards_x64[0] = 0.375 * 2^64.
    //  4) Bob's rewards_debt_x64[0] = Bob's rewards_debts_x64_old[0] + new_bob_rewards = 22624.875 * 2^64 + 10055.5 * 2^64 = 32680.375 * 2^64.

    //  5) new rewards_left_x64[0] = rewards_left_x64[0] - new_emission_x64[0] = 10055.5 * 2^64 - 10055.5 * 2^64 = 0 * 2^64
    //  6) new_reward_vault_balance[0] = rewards_vault_balance[0] - Amount harvested by staker(s) = 10057 - 10056 = 1. (0.375 is locked for Bob and 0.625 is locked for Alice. If no plan to restart this reward stream and all our stakers have claimed all their latest rewards, The creator can consider to withdraw them using "withdraw_funds" ixn, if not harvest on behalf of every staker and withdraw. The official raydium's docs clear states to not call it early.)

    //  Restarting the 1st reward stream for 3 seconds with emission per second to be 10055.625 (2 decimals).
    //  transfer_amount = ceil((10055.625 * 2^64 * 3) / 2^64) = ceil((30166.875 * 2^64 ) / 2^64) = 30167
    //  7) new_rewards_left_x64[0] = rewards_left_x64[0] + transfer_amount = 0 + 30167 * 2^64 = 30167 * 2^64.
    //  8) new_reward_vault_balance[0] = rewards_vault_balance[0] + transfer_amount = 1 + 30167 = 30168.



    let farm_before = get_farm(&svm, &farm_pda);
    let farm_reward_stream_0_before = &farm_before.reward_streams[0];
    
    let farm_reward_0_token_account_before:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();
    
    let bob_ledger_before = get_user_ledger(&svm, &bob_ledger_pda);
    let bob_ledger_reward_stream_0_before = &bob_ledger_before.reward_infos[0];
    
    let bob_reward_0_token_before:TokenAccount = get_spl_account(&svm, &bob_reward_tokens[0]).unwrap();

    
    let farm_reward_stream_1_before = &farm_before.reward_streams[1];
    let farm_reward_1_token_account_before:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();
    let bob_ledger_reward_stream_1_before = &bob_ledger_before.reward_infos[1];
    let bob_reward_1_token_before:TokenAccount = get_spl_account(&svm, &bob_reward_tokens[1]).unwrap();

    

    harvest(&mut svm,HarvestIxn {
        staker:&bob,
        staking_mint:&staking_mint,
        reward_tokens:&bob_reward_tokens
    }).unwrap();

    let farm_after = get_farm(&svm, &farm_pda);
    let farm_reward_stream_0_after = &farm_after.reward_streams[0];
    let farm_reward_stream_1_after = &farm_after.reward_streams[1];

    let farm_reward_0_token_account_after:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();
    
    
    let bob_ledger_after = get_user_ledger(&svm, &bob_ledger_pda);
    let bob_ledger_reward_stream_0_after = &bob_ledger_after.reward_infos[0];
    
    let bob_reward_0_token_after:TokenAccount = get_spl_account(&svm, &bob_reward_tokens[0]).unwrap();

  
    let duration = farm_reward_stream_0_before.end_time.checked_sub(farm_reward_stream_0_before.end_time.min(farm_before.last_updated_time)).unwrap() as u128; 
    assert_eq!(duration,1);

    let new_emissions_x64 = duration.checked_mul(farm_reward_stream_0_before.emission_per_second_x64).unwrap();
    assert_eq!(new_emissions_x64,100555u128.checked_shl(64).unwrap().checked_div(10).unwrap());

    let new_rewards_per_base_unit_x64 = new_emissions_x64.checked_div(farm_before.staked_amount.into()).unwrap();
    assert_eq!(new_rewards_per_base_unit_x64,100555u128.checked_shl(64).unwrap().checked_div(10).unwrap());

    let expected_acc_rewards_per_base_unit_x64 = farm_reward_stream_0_before.acc_rewards_per_base_unit_x64.checked_add(new_rewards_per_base_unit_x64).unwrap();
    assert_eq!(expected_acc_rewards_per_base_unit_x64,32680375u128.checked_shl(64).unwrap().checked_div(1000).unwrap());
    assert_eq!(farm_reward_stream_0_after.acc_rewards_per_base_unit_x64,expected_acc_rewards_per_base_unit_x64);    // - (1)
    
    let expected_farm_rewards_left_x64 = farm_reward_stream_0_before.rewards_left_x64.checked_sub(new_emissions_x64).unwrap();
    assert_eq!(expected_farm_rewards_left_x64,0u128.checked_shl(64).unwrap());
    assert_eq!(farm_reward_stream_0_after.rewards_left_x64,expected_farm_rewards_left_x64);    // - (2)

    let new_bob_rewards_0_x64 = farm_reward_stream_0_after.acc_rewards_per_base_unit_x64.checked_mul(bob_ledger_before.staked_amount.into()).unwrap().checked_sub(bob_ledger_reward_stream_0_before.rewards_debt_x64).unwrap();
    assert_eq!(new_bob_rewards_0_x64, 100555u128.checked_shl(64).unwrap().checked_div(10).unwrap());

    let total_unclaimed_bob_rewards = bob_ledger_reward_stream_0_before.pending_rewards_x64.checked_add(new_bob_rewards_0_x64).unwrap();
    
    let transfered_amount  = total_unclaimed_bob_rewards.checked_shr(64).unwrap() as u64;
    assert_eq!(10056,transfered_amount);
    assert_eq!(bob_reward_0_token_after.amount.checked_sub(bob_reward_0_token_before.amount).unwrap(),transfered_amount);    // - (3)
    assert_eq!(farm_reward_0_token_account_before.amount.checked_sub(farm_reward_0_token_account_after.amount).unwrap(),transfered_amount);
    assert_eq!(farm_reward_0_token_account_after.amount,1);


    let expected_bob_reward_0_pending_rewards_x64 = total_unclaimed_bob_rewards.checked_sub((transfered_amount as u128).checked_shl(64).unwrap()).unwrap();
    assert_eq!(expected_bob_reward_0_pending_rewards_x64,375u128.checked_shl(64).unwrap().checked_div(1000).unwrap());
    assert_eq!(bob_ledger_reward_stream_0_after.pending_rewards_x64,expected_bob_reward_0_pending_rewards_x64);    // - (5)

    let expected_bob_reward_0_rewards_debt_x64 = bob_ledger_reward_stream_0_before.rewards_debt_x64.checked_add(new_bob_rewards_0_x64).unwrap();
    assert_eq!(expected_bob_reward_0_rewards_debt_x64,32680375u128.checked_shl(64).unwrap().checked_div(1000).unwrap());
    assert_eq!(bob_ledger_reward_stream_0_after.rewards_debt_x64,expected_bob_reward_0_rewards_debt_x64);    // - (4)
    
    assert_eq!(farm_reward_stream_0_after.status,RewardStreamStatus::Ended); 

    assert_eq!(farm_reward_stream_1_after.open_time,clock.unix_timestamp);
    assert_eq!(farm_reward_stream_1_after.status,RewardStreamStatus::Running);

    // Like above verify it for reward stream 1
    let farm_reward_1_token_account_after:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();
    let bob_ledger_reward_stream_1_after = &bob_ledger_after.reward_infos[1];
    let bob_reward_1_token_after:TokenAccount = get_spl_account(&svm, &bob_reward_tokens[1]).unwrap();



    let farm_before = get_farm(&svm,&farm_pda);
    let farm_reward_stream_0_before = &farm_before.reward_streams[0];
    let farm_reward_0_token_account_before:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();


    let open_time = clock.unix_timestamp;
    let end_time = clock.unix_timestamp + 3;
    let emission_per_second_x64 = 10055625u128.checked_shl(64).unwrap().checked_div(1000).unwrap();

    restart_rewards(&mut svm, RestartRewardsIxn { creator: &yash, staking_mint: &staking_mint, reward_stream_idx: 0, reward_stream: RewardStreamArgs{
        open_time,
        end_time,
        emission_per_second_x64,
    } }).unwrap();

    let farm_after = get_farm(&svm,&farm_pda);
    let farm_reward_stream_0_after = &farm_after.reward_streams[0];
    let farm_reward_0_token_account_after:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();

    assert_eq!(farm_reward_stream_0_after.acc_rewards_per_base_unit_x64,farm_reward_stream_0_before.acc_rewards_per_base_unit_x64);
    assert_eq!(farm_reward_stream_0_after.emission_per_second_x64,emission_per_second_x64);
    assert_eq!(farm_reward_stream_0_after.open_time,open_time);
    assert_eq!(farm_reward_stream_0_after.end_time,end_time);

    let total_rewards_x64 = 3u128.checked_mul(emission_per_second_x64).unwrap();
    assert!(total_rewards_x64 > farm_reward_stream_0_before.rewards_left_x64);

    let transfer_amount = ceil_div_x64(total_rewards_x64.checked_sub(farm_reward_stream_0_before.rewards_left_x64).unwrap());
    assert_eq!(transfer_amount,30167);
    
    let expected_rewards_left_x64 = farm_reward_stream_0_before.rewards_left_x64.checked_add((transfer_amount as u128).checked_shl(64).unwrap()).unwrap();
    assert_eq!(expected_rewards_left_x64,30167u128.checked_shl(64).unwrap());
    assert_eq!(farm_reward_stream_0_after.rewards_left_x64,expected_rewards_left_x64);

    assert_eq!(farm_reward_0_token_account_after.amount.checked_sub(farm_reward_0_token_account_before.amount).unwrap(),30167);
    assert_eq!(farm_reward_0_token_account_after.amount,30168);




    time_travel(&mut svm,1);
    let clock = svm.get_sysvar::<Clock>();
    assert_eq!(clock.unix_timestamp,6);



    // At t = 6, Bob withdraws all his staked assets
    //  new_emission_x64[0] = (emission_per_second_x64[0] * duration(now,max(open_time,last_update_time))) = 10055.625 * 2^64 * (6 - max(5,5)) = 10055.625 * 2^64 * 1
    //  1) new_acc_rewards_per_base_unit_x64[0]= acc_rewards_per_base_unit_x64[0] + new_emission_x64[0] / total_staked_amount = 32680.375 * 2^64 + (10055.625 * 2^64) / 1 = 42736 * 2^64.
    //  2) new rewards_left_x64[0] = rewards_left_x64[0] - new_emission_x64[0] = 30167 * 2^64 - 10055.625 * 2^64 = 20111.375 * 2^64

    //  new_bob_rewards_x64[0] = new_acc_rewards_per_base_unit_x64[0] * bob.staked_amount_before - rewards_debt_x64_before[0] = 42736 * 2^64 * 1 - 32680.375 * 2^64  = 10055.625 * 2^64
    //  3) Bob's harvested rewards = Bob's pending_rewards_x64[0] + new_bob_rewards_x64[0] = 0.375 * 2^64 + 10055.625 * 2^64 = 10056 * 2^64 >>64 = 10056 is transfered to the token account.
    //  4) Bob's pending_rewards_x64[0] = 0 * 2^64. 
    //  6) Bob's rewards_debt_x64[0] = Bob's rewards_debts_x64_old[0] + new_bob_rewards_x64[0] - withdraw_amount * new_acc_rewards_per_base_unit_x64 = 32680.375 * 2^64 + 10055.625 * 2^64 - 1 * 42736 * 2^64 = 0 * 2^64.


    //  new_emission_x64[1] = (emission_per_second_x64[1] * duration(now,max(open_time,last_update_time))) = 1000000 * 2^ 64 * (6 - max(5,5)) = 1000000 * 2^64 * 1.
    //  1) new_acc_rewards_per_base_unit_x64[1]= acc_rewards_per_base_unit_x64[1] + new_emission_x64[1] / total_staked_amount = 0 + (1000000 * 2^64) / 1 = 1000000 * 2^64.
    //  2) new rewards_left_x64[1] = rewards_left_x64[1] - new_emission_x64[1] = 3000000 * 2^64 - 1000000 * 2^64 = 2000000 * 2^ 64

    //  new_bob_rewards_x64[1] = new_acc_rewards_per_base_unit_x64[1] * bob.staked_amount_before - rewards_debt_x64_before[1] = 1000000 * 2^64 * 1 - 0 * 2^64  = 1000000 * 2^64
    //  3) Bob's harvested rewards = Bob's pending_rewards_x64[1] + new_bob_rewards_x64[1] = 0 * 2^64 + 1000000 * 2^64 = 1000000 * 2^64 >>64 = 1000000 is transfered to the token account.
    //  4) Bob's pending_rewards_x64[1] = 0 * 2^64. 
    //  6) Bob's rewards_debt_x64[1] = Bob's rewards_debts_x64_old[1] + new_bob_rewards_x64[1] - withdraw_amount * new_acc_rewards_per_base_unit_x64 = 0 + 1000000 * 2^64 - 1 * 1000000 * 2^64 = 0 * 2^64.

    let farm_before = get_farm(&svm, &farm_pda);
    let farm_reward_stream_0_before = &farm_before.reward_streams[0];
    let farm_reward_stream_1_before = &farm_before.reward_streams[1];

    let farm_reward_0_token_account_before:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();
    let farm_reward_1_token_account_before:TokenAccount = get_spl_account(&svm, &farm_reward_1_token).unwrap();

    let bob_ledger_before = get_user_ledger(&svm, &bob_ledger_pda);
    let bob_ledger_reward_stream_0_before = &bob_ledger_before.reward_infos[0];
    let bob_ledger_reward_stream_1_before = &bob_ledger_before.reward_infos[1];

    let bob_reward_0_token_before:TokenAccount = get_spl_account(&svm, &bob_reward_tokens[0]).unwrap();
    let bob_reward_1_token_before:TokenAccount = get_spl_account(&svm, &bob_reward_tokens[1]).unwrap();

    unstake(&mut svm,UnstakeIxn { staker: &bob, staking_mint: &staking_mint, staker_staking_token: &bob_staking_ata, reward_tokens: &bob_reward_tokens, withdraw_amount: bob_ledger_before.staked_amount }).unwrap();

    let farm_after = get_farm(&svm, &farm_pda);
    assert_eq!(farm_after.staked_amount,0);
    let farm_reward_stream_0_after = &farm_after.reward_streams[0];
    let farm_reward_stream_1_after = &farm_after.reward_streams[1];

    let farm_reward_0_token_account_after:TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();
    let farm_reward_1_token_account_after:TokenAccount = get_spl_account(&svm, &farm_reward_1_token).unwrap();


    let bob_ledger_after = get_user_ledger(&svm, &bob_ledger_pda);
    let bob_ledger_reward_stream_0_after = &bob_ledger_after.reward_infos[0];
    let bob_ledger_reward_stream_1_after = &bob_ledger_after.reward_infos[1];

    let bob_reward_0_token_after:TokenAccount = get_spl_account(&svm, &bob_reward_tokens[0]).unwrap();
    let bob_reward_1_token_after:TokenAccount = get_spl_account(&svm, &bob_reward_tokens[1]).unwrap();

    let duration = clock.unix_timestamp.checked_sub(farm_before.last_updated_time.max(farm_reward_stream_0_before.open_time)).unwrap() as u128; 
    assert_eq!(duration,1);

    let new_emissions_0_x64 = duration.checked_mul(farm_reward_stream_0_before.emission_per_second_x64).unwrap();
    assert_eq!(new_emissions_0_x64,10055625u128.checked_shl(64).unwrap().checked_div(1000).unwrap());

    let new_rewards_per_base_unit_x64 = new_emissions_0_x64.checked_div(farm_before.staked_amount.into()).unwrap();
    assert_eq!(new_rewards_per_base_unit_x64,10055625u128.checked_shl(64).unwrap().checked_div(1000).unwrap());

    let expected_acc_rewards_per_base_unit_x64 = farm_reward_stream_0_before.acc_rewards_per_base_unit_x64.checked_add(new_rewards_per_base_unit_x64).unwrap();
    assert_eq!(expected_acc_rewards_per_base_unit_x64,42736u128.checked_shl(64).unwrap());
    assert_eq!(farm_reward_stream_0_after.acc_rewards_per_base_unit_x64,expected_acc_rewards_per_base_unit_x64);    // - (1)
    
    let expected_farm_rewards_left_x64 = farm_reward_stream_0_before.rewards_left_x64.checked_sub(new_emissions_0_x64).unwrap();
    assert_eq!(expected_farm_rewards_left_x64,20111375u128.checked_shl(64).unwrap().checked_div(1000).unwrap());
    assert_eq!(farm_reward_stream_0_after.rewards_left_x64,expected_farm_rewards_left_x64);    // - (2)

    let new_bob_rewards_0_x64 = farm_reward_stream_0_after.acc_rewards_per_base_unit_x64.checked_mul(bob_ledger_before.staked_amount.into()).unwrap().checked_sub(bob_ledger_reward_stream_0_before.rewards_debt_x64).unwrap();
    assert_eq!(new_bob_rewards_0_x64, 10055625u128.checked_shl(64).unwrap().checked_div(1000).unwrap());

    let total_unclaimed_bob_rewards = bob_ledger_reward_stream_0_before.pending_rewards_x64.checked_add(new_bob_rewards_0_x64).unwrap();
    
    let transfered_amount  = total_unclaimed_bob_rewards.checked_shr(64).unwrap() as u64;
    assert_eq!(10056,transfered_amount);
    assert_eq!(bob_reward_0_token_after.amount.checked_sub(bob_reward_0_token_before.amount).unwrap(),transfered_amount);    // - (3)
    assert_eq!(farm_reward_0_token_account_before.amount.checked_sub(farm_reward_0_token_account_after.amount).unwrap(),transfered_amount);


    let expected_bob_reward_0_pending_rewards_x64 = total_unclaimed_bob_rewards.checked_sub((transfered_amount as u128).checked_shl(64).unwrap()).unwrap();
    assert_eq!(expected_bob_reward_0_pending_rewards_x64,0u128.checked_shl(64).unwrap());
    assert_eq!(bob_ledger_reward_stream_0_after.pending_rewards_x64,expected_bob_reward_0_pending_rewards_x64);    // - (5)

    let expected_bob_reward_0_rewards_debt_x64 = bob_ledger_reward_stream_0_before.rewards_debt_x64.checked_add(new_bob_rewards_0_x64).unwrap().checked_sub(farm_reward_stream_0_after.acc_rewards_per_base_unit_x64.checked_mul(bob_ledger_before.staked_amount.into()).unwrap()).unwrap();
    assert_eq!(expected_bob_reward_0_rewards_debt_x64,0u128.checked_shl(64).unwrap());
    assert_eq!(bob_ledger_reward_stream_0_after.rewards_debt_x64,expected_bob_reward_0_rewards_debt_x64);    // - (4)
    


    let duration_1 = clock.unix_timestamp.checked_sub(farm_before.last_updated_time.max(farm_reward_stream_1_before.open_time)).unwrap() as u128; 

    let new_emissions_1_x64 = duration_1.checked_mul(farm_reward_stream_1_before.emission_per_second_x64).unwrap();
    assert_eq!(new_emissions_1_x64,1000000u128.checked_shl(64).unwrap());

    let new_rewards_per_base_unit_x64 = new_emissions_1_x64.checked_div(farm_before.staked_amount.into()).unwrap();
    assert_eq!(new_rewards_per_base_unit_x64,1000000u128.checked_shl(64).unwrap());

    let expected_acc_rewards_per_base_unit_x64 = farm_reward_stream_1_before.acc_rewards_per_base_unit_x64.checked_add(new_rewards_per_base_unit_x64).unwrap();
    assert_eq!(expected_acc_rewards_per_base_unit_x64,1000000u128.checked_shl(64).unwrap());
    assert_eq!(farm_reward_stream_1_after.acc_rewards_per_base_unit_x64,expected_acc_rewards_per_base_unit_x64);    // - (1)
    
    let expected_farm_rewards_left_x64 = farm_reward_stream_1_before.rewards_left_x64.checked_sub(new_emissions_1_x64).unwrap();
    assert_eq!(expected_farm_rewards_left_x64,2000000u128.checked_shl(64).unwrap());
    assert_eq!(farm_reward_stream_1_after.rewards_left_x64,expected_farm_rewards_left_x64);    // - (2)

    let new_bob_rewards_1_x64 = farm_reward_stream_1_after.acc_rewards_per_base_unit_x64.checked_mul(bob_ledger_before.staked_amount.into()).unwrap().checked_sub(bob_ledger_reward_stream_1_before.rewards_debt_x64).unwrap();
    assert_eq!(new_bob_rewards_1_x64, 1000000u128.checked_shl(64).unwrap());

    let total_unclaimed_bob_rewards_1 = bob_ledger_reward_stream_1_before.pending_rewards_x64.checked_add(new_bob_rewards_1_x64).unwrap();
    
    let transfered_amount  = total_unclaimed_bob_rewards_1.checked_shr(64).unwrap() as u64;
    assert_eq!(1000000,transfered_amount);
    assert_eq!(bob_reward_1_token_after.amount.checked_sub(bob_reward_1_token_before.amount).unwrap(),transfered_amount);    // - (3)
    assert_eq!(farm_reward_1_token_account_before.amount.checked_sub(farm_reward_1_token_account_after.amount).unwrap(),transfered_amount);


    let expected_bob_reward_1_pending_rewards_x64 = total_unclaimed_bob_rewards_1.checked_sub((transfered_amount as u128).checked_shl(64).unwrap()).unwrap();
    assert_eq!(expected_bob_reward_1_pending_rewards_x64,0u128.checked_shl(64).unwrap());
    assert_eq!(bob_ledger_reward_stream_0_after.pending_rewards_x64,expected_bob_reward_1_pending_rewards_x64);    // - (5)

    let expected_bob_reward_1_rewards_debt_x64 = bob_ledger_reward_stream_1_before.rewards_debt_x64.checked_add(new_bob_rewards_1_x64).unwrap().checked_sub(farm_reward_stream_1_after.acc_rewards_per_base_unit_x64.checked_mul(bob_ledger_before.staked_amount.into()).unwrap()).unwrap();
    assert_eq!(expected_bob_reward_1_rewards_debt_x64,0u128.checked_shl(64).unwrap());
    assert_eq!(bob_ledger_reward_stream_1_after.rewards_debt_x64,expected_bob_reward_1_rewards_debt_x64);    // - (4)

  

    time_travel(&mut svm,2);
    let clock = svm.get_sysvar::<Clock>();
    assert_eq!(clock.unix_timestamp,8);


    // t = 8, Restarting the reward stream 2 for a second with emission rate to be 1500000 tokens / second such that the total_rewards_x64 < rewards_left_x64,
    // the farm refunds the excess to the creator reward token from its vault.
   
    svm.expire_blockhash();
    harvest(&mut svm,HarvestIxn {
        staker:&bob,
        staking_mint:&staking_mint,
        reward_tokens:&bob_reward_tokens
    }).unwrap(); // Invoked to update the farm. No rewards is emitted as the staked amount is 0.
    
    let farm_before = get_farm(&svm,&farm_pda);
    let yash_reward_1_token_account_before:TokenAccount = get_spl_account(&svm,&yash_reward_1_token).unwrap();

    let farm_reward_stream_1_before = &farm_before.reward_streams[1 as usize];
    assert_eq!(farm_reward_stream_1_before.status,RewardStreamStatus::Ended);
    assert_eq!(farm_reward_stream_1_before.rewards_left_x64,2000000u128.checked_shl(64).unwrap());

    let open_time = clock.unix_timestamp;
    let end_time = clock.unix_timestamp + 1;
    let emission_per_second_x64 = 1500000u128.checked_shl(64).unwrap();

    restart_rewards(&mut svm, RestartRewardsIxn { creator: &yash, staking_mint: &staking_mint, reward_stream_idx: 1, reward_stream: RewardStreamArgs{
        open_time,
        end_time,
        emission_per_second_x64,
    } }).unwrap();

    let farm_after = get_farm(&svm,&farm_pda);

    let yash_reward_1_token_account_after:TokenAccount = get_spl_account(&svm,&yash_reward_1_token).unwrap();
    assert_eq!(yash_reward_1_token_account_after.amount.checked_sub(yash_reward_1_token_account_before.amount).unwrap(),500000);

    let farm_reward_stream_1_after = &farm_after.reward_streams[1 as usize];
    assert_eq!(farm_reward_stream_1_after.status,RewardStreamStatus::Running);
    assert_eq!(farm_reward_stream_1_after.rewards_left_x64,1500000u128.checked_shl(64).unwrap());
    assert_eq!(farm_reward_stream_1_after.acc_rewards_per_base_unit_x64,farm_reward_stream_1_before.acc_rewards_per_base_unit_x64);




    time_travel(&mut svm,1);
    let clock = svm.get_sysvar::<Clock>();
    assert_eq!(clock.unix_timestamp,9);

    // At t = 10
    // All the streams are ended and Every staker have claimed their transferrable pending rewards so we can safely withdraw the unclaimed rewards and the dust (accrued loss of rewards of all the stakers due to rounding math) from the reward vaults.

    svm.expire_blockhash();
    harvest(&mut svm,HarvestIxn {
        staker:&bob,
        staking_mint:&staking_mint,
        reward_tokens:&bob_reward_tokens
    }).unwrap(); // Invoked to update the farm. No rewards is emitted as the staked amount is 0.
    
    
    let farm: raydium_farm::Farm = get_farm(&svm,&farm_pda);
    assert_eq!(farm.last_updated_time,clock.unix_timestamp);

    let farm_reward_stream_0 = &farm.reward_streams[0];
    let farm_reward_stream_1 = &farm.reward_streams[1];

    assert_eq!(farm_reward_stream_0.status,RewardStreamStatus::Ended);
    assert_eq!(farm_reward_stream_1.status,RewardStreamStatus::Ended);


    let yash_reward_0_token_account_before:TokenAccount = get_spl_account(&svm,&yash_reward_0_token).unwrap();
    let reward_vault_0_token_account_before: TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();

    withdraw_reward(&mut svm, WithdrawRewardIxn {
        creator:&yash,
        staking_mint:&staking_mint,
        reward_stream_idx:0
    }).unwrap();

    let farm: raydium_farm::Farm = get_farm(&svm,&farm_pda);
    let reward_vault_0_token_account_after: TokenAccount = get_spl_account(&svm, &farm_reward_0_token).unwrap();
    let yash_reward_0_token_account_after:TokenAccount = get_spl_account(&svm,&yash_reward_0_token).unwrap();

    assert!(yash_reward_0_token_account_after.amount >= yash_reward_0_token_account_before.amount.checked_add(reward_vault_0_token_account_before.amount).unwrap());

    assert_eq!(yash_reward_0_token_account_after.amount.checked_sub(yash_reward_0_token_account_before.amount).unwrap(),reward_vault_0_token_account_before.amount.checked_sub(reward_vault_0_token_account_after.amount).unwrap());

    assert_eq!(reward_vault_0_token_account_after.amount,0);
    assert_eq!(farm.reward_streams[0].rewards_left_x64,0);


    let yash_reward_1_token_account_before:TokenAccount = get_spl_account(&svm,&yash_reward_1_token).unwrap();
    let reward_vault_1_token_account_before: TokenAccount = get_spl_account(&svm, &farm_reward_1_token).unwrap();

    withdraw_reward(&mut svm, WithdrawRewardIxn {
        creator:&yash,
        staking_mint:&staking_mint,
        reward_stream_idx:1
    }).unwrap();

    let farm: raydium_farm::Farm = get_farm(&svm,&farm_pda);
    let reward_vault_1_token_account_after: TokenAccount = get_spl_account(&svm, &farm_reward_1_token).unwrap();
    let yash_reward_1_token_account_after:TokenAccount = get_spl_account(&svm,&yash_reward_1_token).unwrap();

    assert!(yash_reward_1_token_account_after.amount >= yash_reward_1_token_account_before.amount.checked_add(reward_vault_1_token_account_before.amount).unwrap());

    assert_eq!(yash_reward_1_token_account_after.amount.checked_sub(yash_reward_1_token_account_before.amount).unwrap(),reward_vault_1_token_account_before.amount.checked_sub(reward_vault_1_token_account_after.amount).unwrap());

    assert_eq!(reward_vault_1_token_account_after.amount,0);
    assert_eq!(farm.reward_streams[1].rewards_left_x64,0);
    
}