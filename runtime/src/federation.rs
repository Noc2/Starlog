//! # Federation Module
//!
//!	The Federation module implements the governance system in form of a Layered Inflationary Time-lock TCR.
//!
//! For more information see https://github.com/PACTCare/Stars-Network/blob/master/WHITEPAPER.md#governance

// TODO: 
// refactor testing pre set accounts

use support::{decl_module, 
	decl_storage, 
	decl_event, 
	StorageMap, 
	ensure,
	traits::{Currency, ExistenceRequirement, WithdrawReason}, 
	dispatch::Result};
use rstd::prelude::*;
use runtime_primitives::traits::{As};
use parity_codec::{Decode, Encode};
use system::ensure_signed;

const ERR_RANK_LOWER: &str = "Candidate already has the maximum rank";
const ERR_RANK_LOCK: &str = "Ranks can only be changed 4 weeks after the last change";

const ERR_VOTE_MIN_STAKE: &str = "To vote you need to stake at least the minimum amount of tokens";
const ERR_VOTE_MIN_LOCK: &str = "To vote you need to lock at least for one week";
const ERR_VOTE_LOCK: &str = "The funds are still locked";
const ERR_VOTE_LOCK_CHALLENGE: &str = "Can't unstake during active challenge";
const ERR_VOTE_RANK: &str = "The intended rank of the candidate needs to be higher than the guest rank.";
const ERR_VOTE_EXIST: &str = "To cancel a vote, you need to have voted for the specific account";
const ERR_VOTE_DOUBLE: &str = "The previous vote needs to be canceled for this, before a new vote can be submitted";

const ERR_OVERFLOW_VOTES: &str = "Overflow adding new votes";
const ERR_OVERFLOW_COUNT: &str = "Overflow increasing vote count";
const ERR_UNDERFLOW: &str = "Underflow removing votes";

const ADMIRAL_RANK: u16 = 5;
const SECTION31_RANK: u16 = 4;
const CAPTAIN_RANK: u16 = 3;
const ENGINEER_RANK: u16 = 2;
const CREW_RANK: u16 = 1;
const GUEST_RANK: u16 = 0;

/// The module's configuration trait.
pub trait Trait: system::Trait + balances::Trait {
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;
}

#[derive(Encode, Decode, Default, Clone, PartialEq)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct Candidate<BlockNumber> {       
    pub current_rank: u16, 
	pub intended_rank: u16, // Same Rank Means Nothing to vote
	pub votes_for: u64, 
	pub votes_against: u64,
	pub last_change: BlockNumber,
	pub challenge_start: BlockNumber,
}

#[derive(Encode, Decode, Default, Clone, PartialEq)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct Vote<Account, Balance, BlockNumber>{
	pub account: Account,
	pub stake_for: Balance, 
	pub stake_against: Balance, 
	pub vote_time: BlockNumber,
	pub lock_time: BlockNumber,
	// challenge id = challenge_start for now
	pub challenge_id: BlockNumber,
}

#[derive(Encode, Decode, Default, Clone, PartialEq)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct ChallengeResult{
	pub success: bool,
	pub executed: bool,
}

decl_storage! {
	trait Store for Module<T: Trait> as FederationModule {
		/// Query by candidate
        CandidateStore get(candidate_by_account): map T::AccountId => Candidate<T::BlockNumber>;

		/// Array of personal votes
        VoteArray get(votes_of_owner_by_index): map (T::AccountId, u64) => Vote<T::AccountId, T::Balance, T::BlockNumber>;

        /// Total count of votes of a user
        VoteCount get(vote_count): map T::AccountId => u64;

        /// Index of specific (user, voted account) combination
        VoteIndex get(vote_index): map (T::AccountId, T::AccountId) => u64;

		/// (candidate, challenge_id) => true = successful challenge, false unsuccessful
		ResultStore get(result): map (T::AccountId, T::BlockNumber) => ChallengeResult;

		// parameters 
		/// Minimum stake requirements for admirals
		pub AdmiralStake get(admiral_stake) config(): u64 = 5000;
		/// Minimum stake requirements for sections31	
		pub Section31Stake get(section31_stake) config(): u64 = 4000;
		/// Minimum stake requirements for captains		
		pub CaptainStake get(captain_stake) config(): u64 = 3000;
		/// Minimum stake requirements for engineers		
		pub EngineerStake get(engineer_stake) config(): u64 = 2000;
		/// Minimum stake requirements for crew members		
		pub CrewStake get(crew_stake) config(): u64 = 1000;

		/// Minimum stake
		pub MinStake get(min_stake) config(): u64 = 100;

		/// Minimum lock up time, one week with 6 seconds blocktime
		pub MinLockTime get(min_lock) config(): u64 = 100800;

		/// After you switched your rank, you can only switch it again after one month
		pub RankLock get(rank_lock) config(): T::BlockNumber = T::BlockNumber::sa(403200);

		/// Lock time after new challenge
		pub ChallengeLock get(challenge_lock) config(): T::BlockNumber = T::BlockNumber::sa(100800);
	}
}

decl_module! {
	/// The module declaration.
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		fn deposit_event<T>() = default;

		/// Change own rank
		/// Similiar to application in classic TCR
        fn apply_for_promotion(origin) -> Result {
			let sender = ensure_signed(origin)?;
			let mut candidate = Self::candidate_by_account(&sender);
			candidate.intended_rank = candidate.current_rank + 1;
			let block_number = <system::Module<T>>::block_number();

			ensure!(block_number - candidate.last_change >= Self::rank_lock() || 
					candidate.last_change == T::BlockNumber::sa(0), ERR_RANK_LOCK);
			ensure!(candidate.intended_rank <= ADMIRAL_RANK, ERR_RANK_LOWER);

			candidate.last_change = block_number;
			let candidate = Self::_return_updated_rank(sender.clone(), candidate.clone());
			<CandidateStore<T>>::insert(sender.clone(), &candidate);
			Self::deposit_event(RawEvent::CandidateStored(sender, candidate.intended_rank));
			Ok(())
		}

		// TODO: Cancel membership

		/// Vote for a candidate
		fn candidate_vote(origin, candidate_id: T::AccountId, stake: T::Balance, lock_time: T::BlockNumber) -> Result {
			let sender = ensure_signed(origin)?;
			let mut candidate = Self::candidate_by_account(&candidate_id);
			let vote_index = Self::vote_index((sender.clone(), candidate_id.clone()));

			Self::_check_vote(candidate.intended_rank, vote_index, lock_time)?;
			Self::_stake(&sender, stake.clone())?;

			//store vote
			let vote_time = <system::Module<T>>::block_number();
			let mut challenge_id = T::BlockNumber::sa(0);
			//if active challenge add challenge id
			if (vote_time - candidate.challenge_start) <= Self::challenge_lock() {
				challenge_id = candidate.challenge_start;
			}
			let vote = Vote {
               	account: candidate_id.clone(),
				stake_for: stake,
				stake_against: T::Balance::sa(0),
				vote_time,
				lock_time,
				challenge_id,
			};

			Self::_store_vote(sender.clone(), candidate_id.clone(), vote.clone())?;

			//update candidate
			candidate.votes_for = candidate.votes_for.checked_add(Self::_calculate_voting_power(stake, lock_time)).ok_or(ERR_OVERFLOW_VOTES)?;
			let candidate = Self::_return_updated_rank(candidate_id.clone(), candidate.clone());
			<CandidateStore<T>>::insert(candidate_id.clone(), &candidate);
			Self::deposit_event(RawEvent::Voted(candidate_id, stake));
			Ok(())
		}

		/// Vote against candidate
		/// TODO: commit-reveal https://github.com/ConsenSys/PLCRVoting
		fn candidate_challenge(origin, candidate_id: T::AccountId, stake: T::Balance, lock_time: T::BlockNumber) -> Result {
			let sender = ensure_signed(origin)?;
			let mut candidate = Self::candidate_by_account(&candidate_id);
			let vote_index = Self::vote_index((sender.clone(), candidate_id.clone()));
			
			Self::_check_vote(candidate.intended_rank, vote_index, lock_time)?;
			Self::_stake(&sender, stake.clone())?;

			let vote_time = <system::Module<T>>::block_number();
			if candidate.votes_against == 0 {
				// if nobody voted against a candidate before, challenge time starts
				candidate.challenge_start = vote_time;
			}

			//store vote
			let vote = Vote {
               	account: candidate_id.clone(),
				stake_for: T::Balance::sa(0),
				stake_against: stake,
				vote_time,
				lock_time,
				challenge_id: vote_time,
			};

			Self::_store_vote(sender.clone(), candidate_id.clone(), vote.clone())?;

			candidate.votes_against = candidate.votes_against.checked_add(Self::_calculate_voting_power(stake, lock_time)).ok_or(ERR_OVERFLOW_VOTES)?;
			<CandidateStore<T>>::insert(candidate_id.clone(), &candidate);
			Self::deposit_event(RawEvent::Challenged(candidate_id, stake));
			Ok(())
		}

		/// Cancel vote for specific account and collect funds
		fn cancel_candidate_vote(origin, candidate_id: T::AccountId) -> Result {
			let sender = ensure_signed(origin)?;
			let vote_index = Self::vote_index((sender.clone(), candidate_id.clone())); //0
			let old_vote = Self::votes_of_owner_by_index((sender.clone(), vote_index));
			let mut candidate = Self::candidate_by_account(&candidate_id);

			let block_number = <system::Module<T>>::block_number();
			let mut result = ChallengeResult {
				success: false,
				executed: false,
			};
			if (block_number - candidate.challenge_start) > Self::challenge_lock() && candidate.votes_against != 0 {
				if candidate.votes_for >= candidate.votes_against {
					<ResultStore<T>>::insert((candidate_id.clone(), candidate.challenge_start), result.clone());
				}
				else {
					result.success = true;
					<ResultStore<T>>::insert((candidate_id.clone(), candidate.challenge_start), result.clone());
				}
				// after challenge time allow new challenge, reset previous runs 
				candidate.votes_against = 0;
				candidate.challenge_start = T::BlockNumber::sa(0);
			}
			else if old_vote.challenge_id != T::BlockNumber::sa(0) {
				result = Self::result((candidate_id.clone(),old_vote.challenge_id));
			}

			// if voted during current challenge time, can't unstake!
			ensure!(candidate.challenge_start == T::BlockNumber::sa(0) || candidate.challenge_start > old_vote.vote_time, ERR_VOTE_LOCK_CHALLENGE);

			Self::_unstake(&sender, &old_vote, result.success)?;
			<VoteArray<T>>::remove((sender.clone(), vote_index)); 
			<VoteIndex<T>>::remove((sender.clone(), candidate_id.clone()));
			
			candidate.votes_for = candidate.votes_for.checked_sub(Self::_calculate_voting_power(old_vote.stake_for, old_vote.lock_time)).ok_or(ERR_UNDERFLOW)?;
			let candidate = Self::_return_updated_rank(candidate_id.clone(), candidate.clone());
			<CandidateStore<T>>::insert(&candidate_id, &candidate);

			Self::deposit_event(RawEvent::Voted(candidate_id, old_vote.stake_for));
			Ok(())
		}
	}
}

decl_event!(
	pub enum Event<T> where 
	<T as system::Trait>::AccountId, 
	<T as balances::Trait>::Balance 
	{
		CandidateStored(AccountId, u16),
		Voted(AccountId, Balance),
		Challenged(AccountId, Balance),
		CancelVote(AccountId, Balance),
	}
);

impl<T: Trait> Module<T> {
	fn _check_vote(intended_rank: u16, vote_index: u64, lock_time: T::BlockNumber) -> Result{
		ensure!(intended_rank > GUEST_RANK, ERR_VOTE_RANK);
		ensure!(lock_time >= T::BlockNumber::sa(Self::min_lock()), ERR_VOTE_MIN_LOCK);
		ensure!(vote_index == 0, ERR_VOTE_DOUBLE);
		Ok(())
	}

	fn _stake(sender: &T::AccountId, stake: T::Balance) -> Result{
		ensure!(stake >= T::Balance::sa(Self::min_stake()), ERR_VOTE_MIN_STAKE);
		let _ = <balances::Module<T> as Currency<_>>::withdraw(
            sender,
            stake,
            WithdrawReason::Reserve,
            ExistenceRequirement::KeepAlive,
        )?;
        Ok(())
	}

	fn _store_vote(sender: T::AccountId,candidate_id: T::AccountId, vote: Vote<T::AccountId, T::Balance, T::BlockNumber>) -> Result{
		let count = Self::vote_count(&sender);
		let updated_count = count.checked_add(1).ok_or(ERR_OVERFLOW_COUNT)?;
		<VoteArray<T>>::insert((sender.clone(), updated_count), &vote);
		<VoteIndex<T>>::insert((sender.clone(), candidate_id.clone()), updated_count);
		<VoteCount<T>>::insert(&sender, updated_count);
		Ok(())
	}

	fn _calculate_voting_power(stake: T::Balance, lock_time: T::BlockNumber) -> u64 {
		let voting_power = stake.as_()/&Self::min_stake() * lock_time.as_()/&Self::min_lock() * lock_time.as_()/&Self::min_lock();
		voting_power
	}

	fn _unstake(sender: &T::AccountId, old_vote: &Vote<T::AccountId, T::Balance, T::BlockNumber>, challenge_result: bool) -> Result{
		let mut stake = old_vote.stake_for;
		let mut voted_against = false;
		if old_vote.stake_against == T::Balance::sa(0) {
			ensure!(old_vote.stake_for >=  T::Balance::sa(Self::min_stake()), ERR_VOTE_EXIST);
		}
		else {
			voted_against = true;
			stake = old_vote.stake_against;
			ensure!(old_vote.stake_against >=  T::Balance::sa(Self::min_stake()), ERR_VOTE_EXIST);
		}
		let block_number = <system::Module<T>>::block_number();
		let block_dif = block_number - old_vote.vote_time;

		// you have only access to your money after lock_time
		ensure!(block_dif > old_vote.lock_time, ERR_VOTE_LOCK);

		let mut earned_money = stake;
		// instead of slashing you just don't earn the inflation
		if voted_against == challenge_result {
			// 10% income per year with 1 Block per 6 seconds 
			earned_money = (T::Balance::sa(block_dif.as_() * stake.as_() * 195069/10000000000000)) + stake;
		}
		let _ = <balances::Module<T> as Currency<_>>::deposit_into_existing(sender, earned_money)?;
		Ok(())
	}

	/// Returns the updated rank
	fn _return_updated_rank(candidate_id: T::AccountId, mut candidate: Candidate<T::BlockNumber>) -> Candidate<T::BlockNumber>{
		let block_number = <system::Module<T>>::block_number();
		// Don't change rank during active challenge time
		if (block_number - candidate.challenge_start) >= Self::challenge_lock() {
			// get last challenge result
			let mut result = Self::result((candidate_id.clone(), candidate.challenge_start));
			// in the case of an successful challenge, loose ranks and stake
			if candidate.challenge_start != T::BlockNumber::sa(0) && result.success && !result.executed {
				candidate.current_rank = GUEST_RANK;
				candidate.votes_for = 0;
				result.executed = true;
				<ResultStore<T>>::insert((candidate_id.clone(), candidate.challenge_start), result);
			}
			else {
				if candidate.votes_for > Self::admiral_stake() && candidate.intended_rank == ADMIRAL_RANK {
					candidate.current_rank = ADMIRAL_RANK;
				} else if candidate.votes_for > Self::section31_stake() && candidate.intended_rank == SECTION31_RANK {
					candidate.current_rank = SECTION31_RANK;
				} else if candidate.votes_for > Self::captain_stake() && candidate.intended_rank == CAPTAIN_RANK {
					candidate.current_rank = CAPTAIN_RANK;
				} else if candidate.votes_for > Self::engineer_stake() && candidate.intended_rank == ENGINEER_RANK {
					candidate.current_rank = ENGINEER_RANK;
				} else if candidate.votes_for > Self::crew_stake() && candidate.intended_rank == CREW_RANK {
					candidate.current_rank = CREW_RANK;
				} 
			}
		}

		candidate
	}
}

/// tests for this module
#[cfg(test)]
mod tests {
	use super::*;

	use runtime_io::with_externalities;
	use primitives::{H256, Blake2Hasher};
	use support::{impl_outer_origin, assert_noop, assert_ok};
	use runtime_primitives::{
		BuildStorage,
		traits::{BlakeTwo256, IdentityLookup},
		testing::{Digest, DigestItem, Header}
	};

	const ERR_BALANCE_LOW: &str = "too few free funds in account";

	impl_outer_origin! {
		pub enum Origin for Test {}
	}

	// For testing the module, we construct most of a mock runtime. This means
	// first constructing a configuration type (`Test`) which `impl`s each of the
	// configuration traits of modules we want to use.
	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	impl system::Trait for Test {
		type Origin = Origin;
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type Digest = Digest;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = ();
		type Log = DigestItem;
	}

	impl balances::Trait for Test {
        type Balance = u64;
        type OnFreeBalanceZero = ();
        type OnNewAccount = ();
        type Event = ();
        type TransactionPayment = ();
        type DustRemoval = ();
        type TransferPayment = ();
    }

	impl Trait for Test {
		type Event = ();
	}

	type Balances = balances::Module<Test>;
	type System = system::Module<Test>;
	type FederationModule = Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	fn new_test_ext() -> runtime_io::TestExternalities<Blake2Hasher> {
		system::GenesisConfig::<Test>::default().build_storage().unwrap().0.into()
	}


	#[test]
	fn apply_for_promotion_works() {
		with_externalities(&mut new_test_ext(), || {
			assert_ok!(FederationModule::apply_for_promotion(Origin::signed(0)));
			assert_noop!(FederationModule::apply_for_promotion(Origin::signed(0)), ERR_RANK_LOCK);			
			let candidate = FederationModule::candidate_by_account(&0);
			assert_eq!(candidate.intended_rank, 1);
			
			// assert_noop!(FederationModule::apply_for_promotion(Origin::signed(0)), ERR_RANK_LOWER);
		});
	}

	#[test]
	fn candidate_vote_works() {
		with_externalities(&mut new_test_ext(), || {
			let candidate_to_vote: u64 = 2;
			let voter: u64 = 0;
			let stake: u64 = 7001;
			let lock: u64 = 1000000;

			assert_noop!(FederationModule::candidate_vote(Origin::signed(1), 1, stake.clone(), lock.clone()), ERR_VOTE_RANK);
			System::set_block_number(500000);
			let _ = FederationModule::apply_for_promotion(Origin::signed(candidate_to_vote.clone()));
			assert_noop!(
                FederationModule::candidate_vote(Origin::signed(1), candidate_to_vote.clone(), stake.clone(), lock.clone()),
                ERR_BALANCE_LOW
            );
			let _ = Balances::make_free_balance_be(&voter, 200000);
			assert_noop!(
                FederationModule::candidate_vote(Origin::signed(voter.clone()), candidate_to_vote.clone(), 5, lock.clone()),
                ERR_VOTE_MIN_STAKE
            );
			assert_noop!(
                FederationModule::candidate_vote(Origin::signed(voter.clone()), candidate_to_vote.clone(), stake.clone(), 5),
                ERR_VOTE_MIN_LOCK
            );
			assert_ok!(FederationModule::candidate_vote(Origin::signed(voter.clone()), candidate_to_vote.clone(), stake.clone(), lock.clone()));
			assert_noop!(FederationModule::candidate_vote(Origin::signed(voter.clone()), candidate_to_vote.clone(), stake.clone(), lock.clone()),
				ERR_VOTE_DOUBLE
			);
			let candidate = FederationModule::candidate_by_account(candidate_to_vote.clone());
			assert_eq!(candidate.votes_for, 6884);
			assert_eq!(candidate.current_rank, 1);
			let vote = FederationModule::votes_of_owner_by_index((voter.clone(), 1));
			assert_eq!(vote.stake_for, stake);
		});
	}

	#[test]
	fn candidate_challenge_works() {
		with_externalities(&mut new_test_ext(), || {
			let candidate_to_challenge: u64 = 2;
			let voter: u64 = 0;
			let stake: u64 = 7001;
			let lock: u64 = 1000000;
			// TODO: test all scenarios
			assert_noop!(FederationModule::candidate_challenge(Origin::signed(1), 1, stake.clone(), lock.clone()), ERR_VOTE_RANK);
			System::set_block_number(500000);
			let _ = FederationModule::apply_for_promotion(Origin::signed(candidate_to_challenge.clone()));
			assert_noop!(FederationModule::candidate_challenge(Origin::signed(1), candidate_to_challenge.clone(), stake.clone(), lock.clone()), ERR_BALANCE_LOW);
			let _ = Balances::make_free_balance_be(&voter, 200000);
			assert_ok!(FederationModule::candidate_challenge(Origin::signed(voter.clone()), candidate_to_challenge.clone(), stake.clone(), lock.clone()));	
			let candidate = FederationModule::candidate_by_account(candidate_to_challenge.clone());
			assert_eq!(candidate.votes_against, 6884);	
		});
	}

	#[test]
	fn cancel_candidate_vote_works() {
		with_externalities(&mut new_test_ext(), || {
			let candidate_to_vote: u64 = 2;
			let voter: u64 = 0;
			let challenger: u64=1;

			assert_noop!(FederationModule::cancel_candidate_vote(Origin::signed(voter.clone()), candidate_to_vote.clone()), ERR_VOTE_EXIST);
			let _ = Balances::make_free_balance_be(&voter, 2000);
			System::set_block_number(500000);
			let _ = FederationModule::apply_for_promotion(Origin::signed(candidate_to_vote.clone()));
			assert_ok!(FederationModule::candidate_vote(Origin::signed(voter.clone()), candidate_to_vote.clone(), 1000, 200000));
			// 5126400 Blocks per year -> 10 % income per year
			assert_noop!(FederationModule::cancel_candidate_vote(Origin::signed(voter.clone()), candidate_to_vote.clone()), ERR_VOTE_LOCK);	

			System::set_block_number(5626401);
			assert_ok!(FederationModule::cancel_candidate_vote(Origin::signed(voter.clone()), candidate_to_vote.clone()));
			let free_balance = Balances::free_balance(voter.clone());
			let candidate = FederationModule::candidate_by_account(&candidate_to_vote);
			assert_eq!(candidate.votes_for, 0);
			assert_eq!(free_balance, 2100);

			// candidate challenge, same stake
			let _ = Balances::make_free_balance_be(&challenger, 2000);
			let _ = FederationModule::candidate_challenge(Origin::signed(challenger.clone()), candidate_to_vote.clone(), 1000, 200000);
			assert_ok!(FederationModule::candidate_vote(Origin::signed(voter.clone()), candidate_to_vote.clone(), 100, 200000));			
			assert_noop!(FederationModule::cancel_candidate_vote(Origin::signed(challenger.clone()), candidate_to_vote.clone()), ERR_VOTE_LOCK_CHALLENGE);
			System::set_block_number(6926401);
			assert_ok!(FederationModule::cancel_candidate_vote(Origin::signed(challenger.clone()), candidate_to_vote.clone()));
			assert_ok!(FederationModule::cancel_candidate_vote(Origin::signed(voter.clone()), candidate_to_vote.clone()));
			let free_balance_challenger = Balances::free_balance(challenger.clone());
			let free_balance_voter = Balances::free_balance(voter.clone());
			let candidate = FederationModule::candidate_by_account(&candidate_to_vote);
			assert_eq!(candidate.votes_for, 0);
			assert_eq!(free_balance_challenger, 2025);	
			assert_eq!(free_balance_voter, 2100);	
		});
	}
}