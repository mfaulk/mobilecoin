// Copyright (c) 2018-2020 MobileCoin Inc.

//! Validates that a transaction or list of transactions are safe to append to the ledger.
//!
//! Validation is broken into two parts:
//! 1) "Well formed"-ness - A transaction is considered "well formed" if all the data in it that is
//!    not affected by future changes to the ledger is correct. This includes checks like
//!    inputs/outputs counts, range proofs, signature validation, membership proofs, etc.
//!    A transaction that is well-formed remains well-formed if additional transactions are
//!    appended to the ledger. However, a transaction could transition from not well-formed to well-formed:
//!    for example, the transaction may include inputs that are not yet in the local ledger because
//!    the local ledger is out of sync with the consensus ledger.
//!
//! 2) "Is valid [to add to the ledger]" - This checks whether a **single** transaction can be safely
//!  appended to a ledger in it's current state.
//!
//! This definition differs from what the `mc_transaction_core::validation` module - the check provided by
//! it is actually the "Is well formed" check, and might be renamed in the future to match this.

use crate::tx_manager::UntrustedInterfaces as TxManagerUntrustedInterfaces;
use mc_consensus_enclave::{TxContext, WellFormedTxContext};
use mc_crypto_keys::CompressedRistrettoPublic;
use mc_ledger_db::Ledger;
use mc_transaction_core::{
    ring_signature::KeyImage,
    tx::{TxHash, TxOutMembershipProof},
    validation::{validate_tombstone, TransactionValidationError, TransactionValidationResult},
};
use std::{collections::HashSet, iter::FromIterator, sync::Arc};

#[derive(Clone)]
pub struct DefaultTxManagerUntrustedInterfaces<L: Ledger> {
    ledger: L,
}

impl<L: Ledger + Sync> DefaultTxManagerUntrustedInterfaces<L> {
    pub fn new(ledger: L) -> Self {
        Self { ledger }
    }
}

impl<L: Ledger + Sync> TxManagerUntrustedInterfaces for DefaultTxManagerUntrustedInterfaces<L> {
    /// Performs **only** the non-enclave part of the well-formed check.
    ///
    /// Returns the local ledger's block index and membership proofs for each highest index.
    fn well_formed_check(
        &self,
        tx_context: &TxContext,
    ) -> TransactionValidationResult<(u64, Vec<TxOutMembershipProof>)> {
        // The transaction's membership proofs must reference data contained in the ledger.
        // Note that this check could fail if the local ledger is behind the network's consensus ledger.
        let membership_proofs = self
            .ledger
            .get_tx_out_proof_of_memberships(&tx_context.highest_indices)
            .map_err(|e| TransactionValidationError::Ledger(e.to_string()))?;

        // Note: It is possible that the proofs above are obtained for a different block index as a
        // new block could be written between getting the proofs and the call to num_blocks().
        // However, this has no effect on validation as the block index is only used for tombstone
        // checking.
        let num_blocks = self
            .ledger
            .num_blocks()
            .map_err(|e| TransactionValidationError::Ledger(e.to_string()))?;

        Ok((num_blocks - 1, membership_proofs))
    }

    /// Checks if a transaction is valid (see definition at top of this file).
    fn is_valid(&self, context: Arc<WellFormedTxContext>) -> TransactionValidationResult<()> {
        let current_block_index = self
            .ledger
            .num_blocks()
            .map_err(|e| TransactionValidationError::Ledger(e.to_string()))?;

        // The transaction must not have expired.
        validate_tombstone(current_block_index, context.tombstone_block())?;

        // The `key_images` must not have already been spent.
        let key_images = context.key_images();
        if key_images
            .iter()
            .any(|key_image| self.ledger.contains_key_image(key_image).unwrap_or(true))
        {
            // At least one key image was spent, or the ledger returned an error.
            return Err(TransactionValidationError::ContainsSpentKeyImage);
        }

        // The `output_public_keys` must not appear in the ledger.
        let contains_existing_public_key = context.output_public_keys().iter().any(|public_key| {
            self.ledger
                .contains_tx_out_public_key(public_key)
                .unwrap_or(true)
        });
        if contains_existing_public_key {
            // At least one public key is already in the ledger, or the ledger returned an error.
            return Err(TransactionValidationError::ContainsExistingOutputPublicKey);
        }

        // `tx` is safe to append.
        Ok(())
    }

    /// Combines a set of "candidate values" into a "composite value".
    /// This assumes all values are well-formed and safe to append to the ledger individually.
    ///
    /// # Arguments
    /// * `tx_contexts` - "Candidate" transactions. Each is assumed to be individually valid.
    /// * `max_elements` - Maximum number of elements to return.
    ///
    /// Returns a bounded, deterministically-ordered list of transactions that are safe to append to the ledger.
    fn combine(
        &self,
        tx_contexts: &[Arc<WellFormedTxContext>],
        max_elements: usize,
    ) -> Vec<TxHash> {
        // WellFormedTxContext defines the sort order of transactions within a block.
        let mut candidates: Vec<_> = tx_contexts.to_vec();
        candidates.sort();

        // Allow transactions that do not cause duplicate key images or output public keys.
        let mut allowed_hashes = Vec::new();
        let mut used_key_images: HashSet<&KeyImage> = HashSet::default();
        let mut used_output_public_keys: HashSet<&CompressedRistrettoPublic> = HashSet::default();

        for candidate in &candidates {
            // Enforce maximum size.
            if allowed_hashes.len() >= max_elements {
                break;
            }

            // Reject a transaction that includes a previously used key image.
            let key_images: HashSet<&KeyImage> = HashSet::from_iter(candidate.key_images());
            if !used_key_images.is_disjoint(&key_images) {
                continue;
            }

            // Reject a transaction that includes a previously used output public key.
            let output_public_keys = HashSet::from_iter(candidate.output_public_keys());
            if !used_output_public_keys.is_disjoint(&output_public_keys) {
                continue;
            }

            // The transaction is allowed.
            allowed_hashes.push(*candidate.tx_hash());
            used_key_images.extend(&key_images);
            used_output_public_keys.extend(&output_public_keys);
        }

        allowed_hashes
    }
}

#[cfg(test)]
pub mod well_formed_tests {
    use super::*;
    use mc_common::logger::{test_with_logger, Logger};
    use mc_transaction_core_test_utils::{
        create_ledger, create_transaction, initialize_ledger, AccountKey,
    };
    use rand::SeedableRng;
    use rand_hc::Hc128Rng;

    #[test_with_logger]
    // `is_well_formed` should accept a well-formed transaction.
    fn is_well_formed_accepts_well_formed_transaction(_logger: Logger) {
        let mut rng = Hc128Rng::from_seed([77u8; 32]);

        let sender = AccountKey::random(&mut rng);
        let recipient = AccountKey::random(&mut rng);

        let mut ledger = create_ledger();
        let n_blocks = 3;
        initialize_ledger(&mut ledger, n_blocks, &sender, &mut rng);

        // Choose a TxOut to spend. Only the TxOut in the last block is unspent.
        let block_contents = ledger.get_block_contents(n_blocks - 1).unwrap();
        let tx_out = block_contents.outputs[0].clone();

        let tx = create_transaction(
            &mut ledger,
            &tx_out,
            &sender,
            &recipient.default_subaddress(),
            n_blocks + 1,
            &mut rng,
        );

        let untrusted = DefaultTxManagerUntrustedInterfaces::new(ledger.clone());

        // This tx_context contains highest_indices that exceed the number of TxOuts in the ledger.
        let tx_context = TxContext {
            locally_encrypted_tx: Default::default(),
            tx_hash: tx.tx_hash(),
            highest_indices: tx.get_membership_proof_highest_indices(),
            key_images: tx.key_images(),
            output_public_keys: tx.output_public_keys(),
        };

        match untrusted.well_formed_check(&tx_context) {
            Ok((current_block_index, _highest_index_proofs)) => {
                assert_eq!(current_block_index, n_blocks - 1);
                // TODO: check returned membership proofs.
            }
            Err(e) => panic!("Unexpected error {}", e),
        }
    }

    #[test_with_logger]
    /// `is_well_formed` should reject a transaction that contains a proof-of-membership with
    /// highest index outside the ledger, i.e. a transaction "from the future".
    fn is_well_formed_rejects_excessive_highest_index(_logger: Logger) {
        let mut rng = Hc128Rng::from_seed([77u8; 32]);

        let sender = AccountKey::random(&mut rng);
        // let recipient = AccountKey::random(&mut rng);

        // // Create a TxOut that contains a proof-of-membership whose highest index is outside the ledger.
        // //
        // // An easy way to do this is to populate two ledgers of different lengths with the same randomness.
        // let tx = {
        //     let mut rng = Hc128Rng::from_seed([142u8; 32]);
        //     let mut future_ledger = create_ledger();
        //     let n_blocks = 10;
        //     initialize_ledger(&mut future_ledger, n_blocks, &sender, &mut rng);
        //
        //     // Choose a TxOut to spend. Only the TxOut in the last block is unspent.
        //     let block_contents = future_ledger.get_block_contents(n_blocks - 1).unwrap();
        //     let tx_out = block_contents.outputs[0].clone();
        //
        //     create_transaction(
        //         &mut future_ledger,
        //         &tx_out,
        //         &sender,
        //         &recipient.default_subaddress(),
        //         n_blocks + 1,
        //         &mut rng,
        //     )
        // };

        // The local ledger.
        let mut ledger = create_ledger();
        let n_blocks = 3;
        initialize_ledger(&mut ledger, n_blocks, &sender, &mut rng);

        let untrusted = DefaultTxManagerUntrustedInterfaces::new(ledger.clone());

        // This tx_context contains highest_indices that exceed the number of TxOuts in the ledger.
        let mut tx_context = TxContext::default();
        tx_context.highest_indices = vec![99, 10002, 445];

        match untrusted.well_formed_check(&tx_context) {
            Ok((_cur_block_index, _membership_proofs)) => {
                panic!();
            }
            Err(_e) => {} // This is expected. TODO: check error type.
        }
    }

    // #[test_with_logger]
    // #[ignore]
    // /// `is_well_formed` should reject a transaction with a tombstone block too far in the future.
    // /// Unclear if this should actually be part of the untrusted is_well_formed check...
    // fn is_well_formed_rejects_too_far_tombstone_block(_logger: Logger) {
    //     let mut rng = Hc128Rng::from_seed([79u8; 32]);
    //
    //     let sender = AccountKey::random(&mut rng);
    //     let recipient = AccountKey::random(&mut rng);
    //
    //     let mut ledger = create_ledger();
    //     let n_blocks = 3;
    //     initialize_ledger(&mut ledger, n_blocks, &sender, &mut rng);
    //
    //     let untrusted = DefaultTxManagerUntrustedInterfaces::new(ledger.clone());
    //
    //     // Choose a TxOut to spend. Only the output of the last block is unspent.
    //     let block_contents = ledger.get_block_contents(n_blocks - 1).unwrap();
    //     let tx_out = block_contents.outputs[0].clone();
    //
    //     // A well-formed transaction
    //     {
    //         let tx = create_transaction(
    //             &mut ledger,
    //             &tx_out,
    //             &sender,
    //             &recipient.default_subaddress(),
    //             n_blocks + MAX_TOMBSTONE_BLOCKS,
    //             &mut rng,
    //         );
    //
    //         let membership_proof_highest_indices = tx.get_membership_proof_highest_indices();
    //         assert!(untrusted
    //             .well_formed_check(&membership_proof_highest_indices[..])
    //             .is_ok());
    //     }
    //
    //     // Tombstone block is too far in the future. This is not well-formed.
    //     {
    //         let bad_tx = create_transaction(
    //             &mut ledger,
    //             &tx_out,
    //             &sender,
    //             &recipient.default_subaddress(),
    //             n_blocks + MAX_TOMBSTONE_BLOCKS + 1,
    //             &mut rng,
    //         );
    //
    //         let membership_proof_highest_indices = bad_tx.get_membership_proof_highest_indices();
    //         assert!(untrusted
    //             .well_formed_check(&membership_proof_highest_indices[..])
    //             .is_err());
    //     }
    // }

    // #[allow(soft_unstable)]
    // #[bench_with_logger]
    // #[ignore]
    // fn bench_is_well_formed(_logger: Logger, b: &mut Bencher) {
    //     let mut rng = Hc128Rng::from_seed([79u8; 32]);
    //
    //     let sender = AccountKey::random(&mut rng);
    //     let recipient = AccountKey::random(&mut rng);
    //
    //     let mut ledger = create_ledger();
    //     let n_blocks = 3;
    //     initialize_ledger(&mut ledger, n_blocks, &sender, &mut rng);
    //
    //     // Choose a TxOut to spend. Only the output of the last block is unspent.
    //     let block_contents = ledger.get_block_contents(n_blocks - 1).unwrap();
    //     let tx_out = block_contents.outputs[0].clone();
    //
    //     let tx = create_transaction(
    //         &mut ledger,
    //         &tx_out,
    //         &sender,
    //         &recipient.default_subaddress(),
    //         n_blocks + 10,
    //         &mut rng,
    //     );
    //
    //     b.iter(|| is_well_formed(&tx, &ledger).unwrap())
    // }

    /*
    #[bench]
    #[ignore]
    fn bench_is_well_formed_with_enclave(b: &mut Bencher) {
        const ENCLAVE_FILE: &str = "../libconsensus-enclave.signed.so";
        let enclave_path = env::current_exe()
            .expect("Could not get the path of our executable")
            .with_file_name(ENCLAVE_FILE);
        let enclave = ConsensusServiceSgxEnclave::new(enclave_path);

        let mut rng = Hc128Rng::from_seed([79u8; 32]);

        let sender = AccountKey::random(&mut rng);
        let recipient = AccountKey::random(&mut rng);

        let mut ledger = create_ledger();
        let n_blocks = 3;
        initialize_ledger(&mut ledger, n_blocks, &sender, &mut rng);

        // Choose a TxOut to spend. Only the output of the last block is unspent.
        let mut transactions = ledger.get_transactions_by_block(n_blocks - 1).unwrap();
        let tx_stored = transactions.pop().unwrap();
        let tx_out = tx_stored.outputs[0].clone();

        let tx = create_transaction(
            &mut ledger,
            &tx_out,
            tx_stored.public_key,
            0,
            &sender,
            recipient.default_subaddress(),
            n_blocks + 10,
            &mut rng,
        );

        b.iter(|| is_well_formed_with_enclave(&tx, &enclave, &ledger).unwrap())
    }
    */
}

#[cfg(test)]
mod is_valid_tests {
    use super::*;
    use mc_ledger_db::LedgerDB;
    use mc_transaction_core::{tx::Tx, validation::TransactionValidationError};
    use mc_transaction_core_test_utils::{
        create_ledger, create_transaction, initialize_ledger, AccountKey,
    };
    use rand::SeedableRng;
    use rand_hc::Hc128Rng;

    fn is_valid(tx: &Tx, ledger: &LedgerDB) -> TransactionValidationResult<()> {
        let untrusted = DefaultTxManagerUntrustedInterfaces::new(ledger.clone());
        untrusted.is_valid(Arc::new(WellFormedTxContext::from(tx)))
    }

    #[test]
    /// `is_valid` should reject a transaction with a tombstone block that has been exceeded.
    fn is_valid_rejects_past_tombstone_block() {
        let mut rng = Hc128Rng::from_seed([79u8; 32]);

        let sender = AccountKey::random(&mut rng);
        let recipient = AccountKey::random(&mut rng);

        let mut ledger = create_ledger();
        let n_blocks = 3;
        initialize_ledger(&mut ledger, n_blocks, &sender, &mut rng);

        // Choose a TxOut to spend. Only the output of the last block is unspent.
        let block_contents = ledger.get_block_contents(n_blocks - 1).unwrap();
        let tx_out = block_contents.outputs[0].clone();

        let tx = create_transaction(
            &mut ledger,
            &tx_out,
            &sender,
            &recipient.default_subaddress(),
            n_blocks,
            &mut rng,
        );
        assert_eq!(
            Err(TransactionValidationError::TombstoneBlockExceeded),
            is_valid(&tx, &ledger)
        );

        let tx = create_transaction(
            &mut ledger,
            &tx_out,
            &sender,
            &recipient.default_subaddress(),
            n_blocks - 1,
            &mut rng,
        );
        assert_eq!(
            Err(TransactionValidationError::TombstoneBlockExceeded),
            is_valid(&tx, &ledger)
        );

        let tx = create_transaction(
            &mut ledger,
            &tx_out,
            &sender,
            &recipient.default_subaddress(),
            0,
            &mut rng,
        );
        assert_eq!(
            Err(TransactionValidationError::TombstoneBlockExceeded),
            is_valid(&tx, &ledger)
        );
    }

    #[test]
    /// `is_valid` should reject a transaction with an already spent key image.
    fn is_valid_rejects_spent_keyimage() {
        let mut rng = Hc128Rng::from_seed([79u8; 32]);

        let sender = AccountKey::random(&mut rng);
        let recipient = AccountKey::random(&mut rng);

        let mut ledger = create_ledger();
        let n_blocks = 3;
        initialize_ledger(&mut ledger, n_blocks, &sender, &mut rng);

        // Choose a TxOut to spend. Only the output of the last block is unspent.
        let block_contents = ledger.get_block_contents(n_blocks - 1).unwrap();
        let tx_out = block_contents.outputs[0].clone();

        let mut tx = create_transaction(
            &mut ledger,
            &tx_out,
            &sender,
            &recipient.default_subaddress(),
            n_blocks + 5,
            &mut rng,
        );

        tx.signature.ring_signatures[0].key_image = block_contents.key_images[0].clone();
        assert_eq!(
            Err(TransactionValidationError::ContainsSpentKeyImage),
            is_valid(&tx, &ledger)
        );
    }

    #[test]
    /// `is_valid` should reject a transaction with an already used output public key.
    fn is_valid_rejects_non_unique_output_public_key() {
        let mut rng = Hc128Rng::from_seed([79u8; 32]);

        let sender = AccountKey::random(&mut rng);
        let recipient = AccountKey::random(&mut rng);

        let mut ledger = create_ledger();
        let n_blocks = 3;
        initialize_ledger(&mut ledger, n_blocks, &sender, &mut rng);

        // Choose a TxOut to spend. Only the output of the last block is unspent.
        let block_contents = ledger.get_block_contents(n_blocks - 1).unwrap();
        let tx_out = block_contents.outputs[0].clone();

        let mut tx = create_transaction(
            &mut ledger,
            &tx_out,
            &sender,
            &recipient.default_subaddress(),
            n_blocks + 5,
            &mut rng,
        );

        tx.prefix.outputs[0].public_key = block_contents.outputs[0].public_key.clone();
        assert_eq!(
            Err(TransactionValidationError::ContainsExistingOutputPublicKey),
            is_valid(&tx, &ledger)
        );
    }
}

#[cfg(test)]
mod combine_tests {
    use super::*;
    use mc_crypto_keys::{RistrettoPrivate, RistrettoPublic};
    use mc_ledger_db::test_utils::get_mock_ledger;
    use mc_transaction_core::{
        onetime_keys::recover_onetime_private_key,
        tx::{TxOut, TxOutMembershipProof},
    };
    use mc_transaction_core_test_utils::AccountKey;
    use mc_transaction_std::{InputCredentials, TransactionBuilder};
    use mc_util_from_random::FromRandom;
    use rand::SeedableRng;
    use rand_hc::Hc128Rng;
    use std::convert::TryFrom;

    fn combine(tx_contexts: Vec<WellFormedTxContext>, max_elements: usize) -> Vec<TxHash> {
        let ledger = get_mock_ledger(10);
        let untrusted = DefaultTxManagerUntrustedInterfaces::new(ledger);
        let tx_contexts: Vec<_> = tx_contexts.into_iter().map(Arc::new).collect();
        untrusted.combine(&tx_contexts, max_elements)
    }

    #[test]
    // "Combining" an empty set should return an empty vec.
    fn combine_empty_set() {
        let transaction_set: Vec<WellFormedTxContext> = Vec::default();
        let combined_transactions = combine(transaction_set, 10);
        assert_eq!(combined_transactions.len(), 0);
    }

    #[test]
    // "Combining" a singleton set should return a vec containing the single element.
    fn combine_single_transaction() {
        let mut rng = Hc128Rng::from_seed([1u8; 32]);

        let alice = AccountKey::random(&mut rng);
        let bob = AccountKey::random(&mut rng);

        // Step 1: create a TxOut and the keys for its enclosing transaction. This TxOut will be
        // used as the input for a transaction used in the test.

        // The transaction secret key r and its public key R.
        let tx_secret_key_for_txo = RistrettoPrivate::from_random(&mut rng);

        let tx_out = TxOut::new(
            123,
            &alice.default_subaddress(),
            &tx_secret_key_for_txo,
            Default::default(),
        )
        .unwrap();

        let tx_public_key_for_txo = RistrettoPublic::try_from(&tx_out.public_key).unwrap();

        // Step 2: Alice creates a transaction that sends the full value of `tx_out` to Bob.

        // Create InputCredentials to spend the TxOut.
        let onetime_private_key = recover_onetime_private_key(
            &tx_public_key_for_txo,
            alice.view_private_key(),
            &alice.default_subaddress_spend_private(),
        );

        let ring: Vec<TxOut> = vec![tx_out];
        let membership_proofs: Vec<TxOutMembershipProof> = ring
            .iter()
            .map(|_tx_out| {
                // TODO: provide valid proofs for each tx_out.
                TxOutMembershipProof::new(0, 0, Default::default())
            })
            .collect();

        let input_credentials = InputCredentials::new(
            ring,
            membership_proofs,
            0,
            onetime_private_key,
            *alice.view_private_key(),
        )
        .unwrap();

        let mut transaction_builder = TransactionBuilder::new();
        transaction_builder.add_input(input_credentials);
        transaction_builder.set_fee(0);
        transaction_builder
            .add_output(123, &bob.default_subaddress(), None, &mut rng)
            .unwrap();

        let tx = transaction_builder.build(&mut rng).unwrap();
        let client_tx = WellFormedTxContext::from(&tx);

        // "Combining" a singleton set should return a vec containing the single element.
        let combined_transactions = combine(vec![client_tx], 100);
        assert_eq!(combined_transactions.len(), 1);
    }

    #[test]
    // `combine` should enforce a maximum limit on the number of returned items.
    fn combine_max_size() {
        let mut rng = Hc128Rng::from_seed([1u8; 32]);
        let mut transaction_set: Vec<WellFormedTxContext> = Vec::new();

        let alice = AccountKey::random(&mut rng);
        let bob = AccountKey::random(&mut rng);

        for _i in 0..10 {
            let client_tx: WellFormedTxContext = {
                // Step 1: create a TxOut and the keys for its enclosing transaction. This TxOut will be
                // used as the input for a transaction used in the test.

                // The transaction keys.
                let tx_secret_key_for_txo = RistrettoPrivate::from_random(&mut rng);

                let tx_out = TxOut::new(
                    88,
                    &alice.default_subaddress(),
                    &tx_secret_key_for_txo,
                    Default::default(),
                )
                .unwrap();

                let tx_public_key_for_txo = RistrettoPublic::try_from(&tx_out.public_key).unwrap();

                // Step 2: Create a transaction that sends the full value of `tx_out` to `recipient_account`.

                let mut transaction_builder = TransactionBuilder::new();

                // Create InputCredentials to spend the TxOut.
                let onetime_private_key = recover_onetime_private_key(
                    &tx_public_key_for_txo,
                    alice.view_private_key(),
                    &alice.default_subaddress_spend_private(),
                );

                // Create InputCredentials to spend the TxOut.
                let ring: Vec<TxOut> = vec![tx_out.clone()];
                let membership_proofs: Vec<TxOutMembershipProof> = ring
                    .iter()
                    .map(|_tx_out| {
                        // TODO: provide valid proofs for each tx_out.
                        TxOutMembershipProof::new(0, 0, Default::default())
                    })
                    .collect();

                let input_credentials = InputCredentials::new(
                    ring,
                    membership_proofs,
                    0,
                    onetime_private_key,
                    *alice.view_private_key(),
                )
                .unwrap();
                transaction_builder.add_input(input_credentials);
                transaction_builder.set_fee(0);
                transaction_builder
                    .add_output(88, &bob.default_subaddress(), None, &mut rng)
                    .unwrap();

                let tx = transaction_builder.build(&mut rng).unwrap();
                WellFormedTxContext::from(&tx)
            };
            transaction_set.push(client_tx);
        }

        let max_elements: usize = 7;
        let combined_transactions = combine(transaction_set, max_elements);

        // The combined list of transactions should contain no more than `max_elements`.
        assert_eq!(combined_transactions.len(), max_elements);
    }

    #[test]
    // `combine` should omit transactions that would cause a key image to be used twice.
    fn combine_reject_reused_key_images() {
        let mut rng = Hc128Rng::from_seed([1u8; 32]);

        let alice = AccountKey::random(&mut rng);
        let bob = AccountKey::random(&mut rng);

        // Create a TxOut that was sent to Alice.
        let tx_out = TxOut::new(
            123,
            &alice.default_subaddress(),
            &RistrettoPrivate::from_random(&mut rng),
            Default::default(),
        )
        .unwrap();

        // Alice creates InputCredentials to spend her tx_out.
        let onetime_private_key = recover_onetime_private_key(
            &RistrettoPublic::try_from(&tx_out.public_key).unwrap(),
            alice.view_private_key(),
            &alice.default_subaddress_spend_private(),
        );

        // Create a transaction that sends the full value of  `tx_out` to bob.
        let first_client_tx: WellFormedTxContext = {
            let ring = vec![tx_out.clone()];
            let membership_proofs: Vec<TxOutMembershipProof> = ring
                .iter()
                .map(|_tx_out| {
                    // TODO: provide valid proofs for each tx_out.
                    TxOutMembershipProof::new(0, 0, Default::default())
                })
                .collect();

            let input_credentials = InputCredentials::new(
                ring,
                membership_proofs,
                0,
                onetime_private_key,
                *alice.view_private_key(),
            )
            .unwrap();

            let mut transaction_builder = TransactionBuilder::new();
            transaction_builder.add_input(input_credentials);
            transaction_builder.set_fee(0);
            transaction_builder
                .add_output(123, &bob.default_subaddress(), None, &mut rng)
                .unwrap();

            let tx = transaction_builder.build(&mut rng).unwrap();
            WellFormedTxContext::from(&tx)
        };

        // Create another transaction that attempts to spend `tx_out`.
        let second_client_tx: WellFormedTxContext = {
            let recipient_account = AccountKey::random(&mut rng);
            let ring: Vec<TxOut> = vec![tx_out];
            let membership_proofs: Vec<TxOutMembershipProof> = ring
                .iter()
                .map(|_tx_out| {
                    // TODO: provide valid proofs for each tx_out.
                    TxOutMembershipProof::new(0, 0, Default::default())
                })
                .collect();

            let input_credentials = InputCredentials::new(
                ring,
                membership_proofs,
                0,
                onetime_private_key,
                *alice.view_private_key(),
            )
            .unwrap();

            let mut transaction_builder = TransactionBuilder::new();
            transaction_builder.add_input(input_credentials);
            transaction_builder.set_fee(0);
            transaction_builder
                .add_output(123, &recipient_account.default_subaddress(), None, &mut rng)
                .unwrap();

            let tx = transaction_builder.build(&mut rng).unwrap();
            WellFormedTxContext::from(&tx)
        };

        // This transaction spends a different TxOut, unrelated to `first_client_tx` and `second_client_tx`.
        let third_client_tx: WellFormedTxContext = {
            let recipient_account = AccountKey::random(&mut rng);

            // The transaction keys.
            let tx_secret_key_for_txo = RistrettoPrivate::from_random(&mut rng);
            let tx_out = TxOut::new(
                123,
                &alice.default_subaddress(),
                &tx_secret_key_for_txo,
                Default::default(),
            )
            .unwrap();
            let tx_public_key_for_txo = RistrettoPublic::try_from(&tx_out.public_key).unwrap();

            // Step 2: Create a transaction that sends the full value of `tx_out` to `recipient_account`.

            // Create InputCredentials to spend the TxOut.
            let onetime_private_key = recover_onetime_private_key(
                &tx_public_key_for_txo,
                alice.view_private_key(),
                &alice.default_subaddress_spend_private(),
            );

            let ring: Vec<TxOut> = vec![tx_out];
            let membership_proofs: Vec<TxOutMembershipProof> = ring
                .iter()
                .map(|_tx_out| {
                    // TODO: provide valid proofs for each tx_out.
                    TxOutMembershipProof::new(0, 0, Default::default())
                })
                .collect();

            let input_credentials = InputCredentials::new(
                ring,
                membership_proofs,
                0,
                onetime_private_key,
                *alice.view_private_key(),
            )
            .unwrap();

            let mut transaction_builder = TransactionBuilder::new();
            transaction_builder.add_input(input_credentials);
            transaction_builder.set_fee(0);
            transaction_builder
                .add_output(123, &recipient_account.default_subaddress(), None, &mut rng)
                .unwrap();

            let tx = transaction_builder.build(&mut rng).unwrap();
            WellFormedTxContext::from(&tx)
        };

        // `combine` the set of transactions.
        let transaction_set = vec![first_client_tx, second_client_tx, third_client_tx.clone()];

        let combined_transactions = combine(transaction_set, 10);
        // `combine` should only allow one of the transactions that attempts to use the same key image.
        assert_eq!(combined_transactions.len(), 2);
        assert!(combined_transactions.contains(third_client_tx.tx_hash()));
    }

    #[test]
    // `combine` should omit transactions that would cause an output public key to appear twice.
    fn combine_reject_duplicate_output_public_key() {
        let mut rng = Hc128Rng::from_seed([1u8; 32]);

        let alice = AccountKey::random(&mut rng);
        let bob = AccountKey::random(&mut rng);

        // Create two TxOuts that were sent to Alice.
        let tx_out1 = TxOut::new(
            123,
            &alice.default_subaddress(),
            &RistrettoPrivate::from_random(&mut rng),
            Default::default(),
        )
        .unwrap();

        let tx_out2 = TxOut::new(
            123,
            &alice.default_subaddress(),
            &RistrettoPrivate::from_random(&mut rng),
            Default::default(),
        )
        .unwrap();

        // Alice creates InputCredentials to spend her tx_outs.
        let onetime_private_key1 = recover_onetime_private_key(
            &RistrettoPublic::try_from(&tx_out1.public_key).unwrap(),
            alice.view_private_key(),
            &alice.default_subaddress_spend_private(),
        );

        let onetime_private_key2 = recover_onetime_private_key(
            &RistrettoPublic::try_from(&tx_out2.public_key).unwrap(),
            alice.view_private_key(),
            &alice.default_subaddress_spend_private(),
        );

        // Create a transaction that sends the full value of  `tx_out1` to bob.
        let first_client_tx: WellFormedTxContext = {
            let ring = vec![tx_out1.clone()];
            let membership_proofs: Vec<TxOutMembershipProof> = ring
                .iter()
                .map(|_tx_out| {
                    // TODO: provide valid proofs for each tx_out.
                    TxOutMembershipProof::new(0, 0, Default::default())
                })
                .collect();

            let input_credentials = InputCredentials::new(
                ring,
                membership_proofs,
                0,
                onetime_private_key1,
                *alice.view_private_key(),
            )
            .unwrap();

            let mut transaction_builder = TransactionBuilder::new();
            transaction_builder.add_input(input_credentials);
            transaction_builder.set_fee(0);
            transaction_builder
                .add_output(123, &bob.default_subaddress(), None, &mut rng)
                .unwrap();

            let tx = transaction_builder.build(&mut rng).unwrap();
            WellFormedTxContext::from(&tx)
        };

        // Create another transaction that attempts to spend `tx_out2` but has the same output
        // public key.
        let second_client_tx: WellFormedTxContext = {
            let recipient_account = AccountKey::random(&mut rng);
            let ring: Vec<TxOut> = vec![tx_out2];
            let membership_proofs: Vec<TxOutMembershipProof> = ring
                .iter()
                .map(|_tx_out| {
                    // TODO: provide valid proofs for each tx_out.
                    TxOutMembershipProof::new(0, 0, Default::default())
                })
                .collect();

            let input_credentials = InputCredentials::new(
                ring,
                membership_proofs,
                0,
                onetime_private_key2,
                *alice.view_private_key(),
            )
            .unwrap();

            let mut transaction_builder = TransactionBuilder::new();
            transaction_builder.add_input(input_credentials);
            transaction_builder.set_fee(0);
            transaction_builder
                .add_output(123, &recipient_account.default_subaddress(), None, &mut rng)
                .unwrap();

            let mut tx = transaction_builder.build(&mut rng).unwrap();
            tx.prefix.outputs[0].public_key = first_client_tx.output_public_keys()[0].clone();
            WellFormedTxContext::from(&tx)
        };

        // This transaction spends a different TxOut, unrelated to `first_client_tx` and `second_client_tx`.
        let third_client_tx: WellFormedTxContext = {
            let recipient_account = AccountKey::random(&mut rng);

            // The transaction keys.
            let tx_secret_key_for_txo = RistrettoPrivate::from_random(&mut rng);
            let tx_out = TxOut::new(
                123,
                &alice.default_subaddress(),
                &tx_secret_key_for_txo,
                Default::default(),
            )
            .unwrap();
            let tx_public_key_for_txo = RistrettoPublic::try_from(&tx_out.public_key).unwrap();

            // Step 2: Create a transaction that sends the full value of `tx_out` to `recipient_account`.

            // Create InputCredentials to spend the TxOut.
            let onetime_private_key = recover_onetime_private_key(
                &tx_public_key_for_txo,
                alice.view_private_key(),
                &alice.default_subaddress_spend_private(),
            );

            let ring: Vec<TxOut> = vec![tx_out];
            let membership_proofs: Vec<TxOutMembershipProof> = ring
                .iter()
                .map(|_tx_out| {
                    // TODO: provide valid proofs for each tx_out.
                    TxOutMembershipProof::new(0, 0, Default::default())
                })
                .collect();

            let input_credentials = InputCredentials::new(
                ring,
                membership_proofs,
                0,
                onetime_private_key,
                *alice.view_private_key(),
            )
            .unwrap();

            let mut transaction_builder = TransactionBuilder::new();
            transaction_builder.add_input(input_credentials);
            transaction_builder.set_fee(0);
            transaction_builder
                .add_output(123, &recipient_account.default_subaddress(), None, &mut rng)
                .unwrap();

            let tx = transaction_builder.build(&mut rng).unwrap();
            WellFormedTxContext::from(&tx)
        };

        // `combine` the set of transactions.
        let transaction_set = vec![first_client_tx, second_client_tx, third_client_tx.clone()];

        let combined_transactions = combine(transaction_set, 10);
        // `combine` should only allow one of the transactions that attempts to use the same output
        // public key.
        assert_eq!(combined_transactions.len(), 2);
        assert!(combined_transactions.contains(third_client_tx.tx_hash()));
    }

    #[test]
    // `combine` should return hashes in the order defined by WellformedTxContext.
    fn combine_sort_order() {
        let a = WellFormedTxContext::new(100, TxHash([1u8; 32]), 0, vec![], vec![], vec![]);
        let b = WellFormedTxContext::new(557, TxHash([2u8; 32]), 0, vec![], vec![], vec![]);
        let c = WellFormedTxContext::new(88, TxHash([3u8; 32]), 0, vec![], vec![], vec![]);

        let tx_contexts = vec![a, b, c];

        let hashes = combine(tx_contexts, 10);
        // Transactions should be ordered from highest fee to lowest fee.
        let expected_hashes = vec![TxHash([2u8; 32]), TxHash([1u8; 32]), TxHash([3u8; 32])];
        assert_eq!(hashes, expected_hashes);
    }
}
