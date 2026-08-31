
use anchor_lang::prelude::*;

#[account]
#[derive(InitSpace,Debug)]
pub struct Farm {
    pub authority:Pubkey, 
    pub staking_mint:Pubkey,
    pub staking_mint_program:Pubkey,
    pub staked_amount: u64,
    pub last_updated_time:i64,
    pub reward_streams_count:u8, // Total rewards stream added, Max 5
    pub reward_streams :[RewardStream;5],
    pub bump: u8
}

#[derive(AnchorSerialize,AnchorDeserialize,Copy,Clone,InitSpace,Debug)]
pub struct RewardStream {
    pub reward_mint:Pubkey,
    pub reward_mint_program:Pubkey,

    pub status:RewardStreamStatus,
    pub open_time:i64,
    pub end_time:i64,
    pub acc_rewards_per_base_unit_x64: u128,
    pub rewards_left_x64:u128,
    pub emission_per_second_x64: u128,
}


#[derive(AnchorSerialize,AnchorDeserialize,Copy,Clone,InitSpace,PartialEq,Eq,Debug)]
pub enum RewardStreamStatus {
    Unused,
    Running,
    Ended
}
impl Farm {
    pub const LEN:usize = 8 + Farm::INIT_SPACE; 
    pub const STATIC_SEED:&[u8] = b"farm";

    pub fn update(&mut self)-> Result<()> {
        let block_timestamp = Clock::get()?.unix_timestamp;
        if self.last_updated_time < block_timestamp  {
            for i in 0..self.reward_streams_count {
                if block_timestamp <  self.reward_streams[i as usize].open_time {
                    self.reward_streams[i as usize].status = RewardStreamStatus::Unused;
                    continue;
                }

                let duration = if self.reward_streams[i as usize].open_time <= block_timestamp &&  block_timestamp < self.reward_streams[i as usize].end_time  {
                    self.reward_streams[i as usize].status = RewardStreamStatus::Running;
                    let duration = block_timestamp.checked_sub(self.reward_streams[i as usize].open_time.max(self.last_updated_time)).unwrap() as u128;
                    duration
                }else {
                    self.reward_streams[i as usize].status = RewardStreamStatus::Ended;
                    let duration = self.reward_streams[i as usize].end_time.checked_sub(self.reward_streams[i as usize].end_time.min(self.last_updated_time)).unwrap() as u128;
                    duration
                };
                
                if self.staked_amount > 0 {
                    let new_emission = duration.checked_mul(self.reward_streams[i as usize].emission_per_second_x64).unwrap();
                    
                    self.reward_streams[i as usize].rewards_left_x64 = self.reward_streams[i as usize].rewards_left_x64.checked_sub(new_emission).unwrap();
                    self.reward_streams[i as usize].acc_rewards_per_base_unit_x64 = self.reward_streams[i as usize].acc_rewards_per_base_unit_x64.checked_add(new_emission.checked_div(self.staked_amount.into()).unwrap()).unwrap();
                }
            }
            self.last_updated_time = block_timestamp;
        }

        Ok(())
    }
}