// Copyright (c) MangoNet Labs Ltd.
// SPDX-License-Identifier: Apache-2.0

use crate::validator_commands::{
    get_validator_summary, MgoValidatorCommand, MgoValidatorCommandResponse,
};
use anyhow::Ok;
use fastcrypto::encoding::{Base64, Encoding};
use shared_crypto::intent::{Intent, IntentMessage};
use mgo_types::crypto::MgoKeyPair;
use mgo_types::transaction::TransactionData;
use mgo_types::{base_types::MgoAddress, crypto::Signature, transaction::Transaction};
use test_cluster::TestClusterBuilder;

#[tokio::test]
async fn test_print_raw_rgp_txn() -> Result<(), anyhow::Error> {
    let test_cluster = TestClusterBuilder::new().build().await;
    let keypair: &MgoKeyPair = test_cluster
        .swarm
        .config()
        .validator_configs
        .first()
        .unwrap()
        .account_key_pair
        .keypair();
    let validator_address: MgoAddress = MgoAddress::from(&keypair.public());
    let mut context = test_cluster.wallet;
    let mgo_client = context.get_client().await?;
    let (_, summary) = get_validator_summary(&mgo_client, validator_address)
        .await?
        .unwrap();
    let operation_cap_id = summary.operation_cap_id;

    // Execute the command and get the serialized transaction data.
    let response = MgoValidatorCommand::DisplayGasPriceUpdateRawTxn {
        sender_address: validator_address,
        new_gas_price: 42,
        operation_cap_id,
        gas_budget: None,
    }
    .execute(&mut context)
    .await?;
    let MgoValidatorCommandResponse::DisplayGasPriceUpdateRawTxn {
        data,
        serialized_data,
    } = response
    else {
        panic!("Expected DisplayGasPriceUpdateRawTxn");
    };

    // Construct the signed transaction and execute it.
    let deserialized_data =
        bcs::from_bytes::<TransactionData>(&Base64::decode(&serialized_data).unwrap())?;
    let signature = Signature::new_secure(
        &IntentMessage::new(Intent::mgo_transaction(), deserialized_data),
        keypair,
    );
    let txn = Transaction::from_data(data, vec![signature]);
    context.execute_transaction_must_succeed(txn).await;
    let (_, summary) = get_validator_summary(&mgo_client, validator_address)
        .await?
        .unwrap();

    // Check that the gas price is updated correctly.
    assert_eq!(summary.next_epoch_gas_price, 42);
    Ok(())
}
