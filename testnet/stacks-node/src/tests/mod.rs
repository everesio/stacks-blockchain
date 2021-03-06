mod neon_integrations;
mod integrations;
mod bitcoin_regtest;
mod mempool;

use stacks::chainstate::stacks::events::{StacksTransactionEvent, STXEventType};
use stacks::chainstate::stacks::{TransactionPayload, StacksTransactionSigner, StacksPublicKey,TransactionPostConditionMode, TransactionSmartContract, TransactionAuth,TransactionVersion, C32_ADDRESS_VERSION_TESTNET_SINGLESIG,
    StacksMicroblockHeader, StacksPrivateKey, TransactionAnchorMode, miner::StacksMicroblockBuilder, db::StacksChainState, StacksBlock, StacksMicroblock,
    TokenTransferMemo, CoinbasePayload, TransactionContractCall, StacksAddress, StacksTransaction, TransactionSpendingCondition};
use stacks::net::StacksMessageCodec;
use stacks::util::hash::{hex_bytes};
use stacks::util::strings::StacksString;
use stacks::vm::{ContractName, ClarityName, Value};
use stacks::vm::types::PrincipalData;
use stacks::address::AddressHashMode;
use stacks::burnchains::BurnchainHeaderHash;
use stacks::core::mempool::MemPoolTxInfo;
use stacks::vm::costs::ExecutionCost;

use std::convert::TryInto;
use rand::RngCore; 
use super::{Config};
use crate::helium::RunLoop;
use super::node::{TESTNET_CHAIN_ID};
use super::burnchains::bitcoin_regtest_controller::ParsedUTXO;

// $ cat /tmp/out.clar 
pub const STORE_CONTRACT: &str =  r#"(define-map store ((key (buff 32))) ((value (buff 32))))
 (define-public (get-value (key (buff 32)))
    (begin
      (print (concat "Getting key " key))
      (match (map-get? store { key: key })
        entry (ok (get value entry))
        (err 0))))
 (define-public (set-value (key (buff 32)) (value (buff 32)))
    (begin
        (print (concat "Setting key " key))
        (map-set store { key: key } { value: value })
        (ok true)))"#;
// ./blockstack-cli --testnet publish 043ff5004e3d695060fa48ac94c96049b8c14ef441c50a184a6a3875d2a000f3 0 0 store /tmp/out.clar

pub const SK_1: &'static str = "a1289f6438855da7decf9b61b852c882c398cff1446b2a0f823538aa2ebef92e01";
pub const SK_2: &'static str = "4ce9a8f7539ea93753a36405b16e8b57e15a552430410709c2b6d65dca5c02e201";
pub const SK_3: &'static str = "cb95ddd0fe18ec57f4f3533b95ae564b3f1ae063dbf75b46334bd86245aef78501";

pub const ADDR_4: &'static str = "ST31DA6FTSJX2WGTZ69SFY11BH51NZMB0ZZ239N96";

lazy_static! {
    pub static ref PUBLISH_CONTRACT: Vec<u8> = make_contract_publish(
        &StacksPrivateKey::from_hex("043ff5004e3d695060fa48ac94c96049b8c14ef441c50a184a6a3875d2a000f3").unwrap(),
        0, 0, "store", STORE_CONTRACT);
}

pub fn serialize_sign_standard_single_sig_tx(payload: TransactionPayload,
                                             sender: &StacksPrivateKey, nonce: u64, fee_rate: u64) -> Vec<u8> {
    serialize_sign_standard_single_sig_tx_anchor_mode(
        payload, sender, nonce, fee_rate, TransactionAnchorMode::OnChainOnly)
}

pub fn serialize_sign_standard_single_sig_tx_anchor_mode(
    payload: TransactionPayload, sender: &StacksPrivateKey, nonce: u64, fee_rate: u64,
    anchor_mode: TransactionAnchorMode) -> Vec<u8> {

    let mut spending_condition = TransactionSpendingCondition::new_singlesig_p2pkh(StacksPublicKey::from_private(sender))
        .expect("Failed to create p2pkh spending condition from public key.");
    spending_condition.set_nonce(nonce);
    spending_condition.set_fee_rate(fee_rate);
    let auth = TransactionAuth::Standard(spending_condition);
    let mut unsigned_tx = StacksTransaction::new(TransactionVersion::Testnet, auth, payload);
    unsigned_tx.anchor_mode = anchor_mode;
    unsigned_tx.post_condition_mode = TransactionPostConditionMode::Allow;
    unsigned_tx.chain_id = TESTNET_CHAIN_ID;

    let mut tx_signer = StacksTransactionSigner::new(&unsigned_tx);
    tx_signer.sign_origin(sender).unwrap();

    let mut buf = vec![];
    tx_signer.get_tx().unwrap().consensus_serialize(&mut buf).unwrap();
    buf
}

pub fn make_contract_publish(sender: &StacksPrivateKey, nonce: u64, fee_rate: u64,
                             contract_name: &str, contract_content: &str) -> Vec<u8> {
    let name = ContractName::from(contract_name);
    let code_body = StacksString::from_string(&contract_content.to_string()).unwrap();

    let payload = TransactionSmartContract { name, code_body };

    serialize_sign_standard_single_sig_tx(payload.into(), sender, nonce, fee_rate)
}

pub fn make_contract_publish_microblock_only(sender: &StacksPrivateKey, nonce: u64, fee_rate: u64,
                             contract_name: &str, contract_content: &str) -> Vec<u8> {
    let name = ContractName::from(contract_name);
    let code_body = StacksString::from_string(&contract_content.to_string()).unwrap();

    let payload = TransactionSmartContract { name, code_body };

    serialize_sign_standard_single_sig_tx_anchor_mode(payload.into(), sender, nonce, fee_rate,
                                                      TransactionAnchorMode::OffChainOnly)
}

pub fn new_test_conf() -> Config {
    
    // secretKey: "b1cf9cee5083f421c84d7cb53be5edf2801c3c78d63d53917aee0bdc8bd160ee01",
    // publicKey: "03e2ed46873d0db820e8c6001aabc082d72b5b900b53b7a1b9714fe7bde3037b81",
    // stacksAddress: "ST2VHM28V9E5QCRD6C73215KAPSBKQGPWTEE5CMQT"

    let mut conf = Config::default();
    conf.node.seed = hex_bytes("0000000000000000000000000000000000000000000000000000000000000000").unwrap();
    conf.add_initial_balance("ST2VHM28V9E5QCRD6C73215KAPSBKQGPWTEE5CMQT".to_string(), 10000);

    let mut rng = rand::thread_rng();
    let mut buf = [0u8; 8];
    rng.fill_bytes(&mut buf);

    let rpc_port = u16::from_be_bytes(buf[0..2].try_into().unwrap())
        .saturating_add(1025) - 1; // use a non-privileged port between 1024 and 65534
    let p2p_port = u16::from_be_bytes(buf[2..4].try_into().unwrap())
        .saturating_add(1025) - 1; // use a non-privileged port between 1024 and 65534
    
    let localhost = "127.0.0.1";
    conf.node.rpc_bind = format!("{}:{}", localhost, rpc_port);
    conf.node.p2p_bind = format!("{}:{}", localhost, p2p_port);
    conf.node.data_url = format!("https://{}:{}", localhost, rpc_port);
    conf.node.p2p_address = format!("{}:{}", localhost, p2p_port);
    conf
}

pub fn to_addr(sk: &StacksPrivateKey) -> StacksAddress {
    StacksAddress::from_public_keys(
        C32_ADDRESS_VERSION_TESTNET_SINGLESIG, &AddressHashMode::SerializeP2PKH, 1, &vec![StacksPublicKey::from_private(sk)])
        .unwrap()
}

pub fn make_stacks_transfer(sender: &StacksPrivateKey, nonce: u64, fee_rate: u64,
                            recipient: &PrincipalData, amount: u64) -> Vec<u8> {
    let payload = TransactionPayload::TokenTransfer(recipient.clone(), amount, TokenTransferMemo([0; 34]));
    serialize_sign_standard_single_sig_tx(payload.into(), sender, nonce, fee_rate)
}

pub fn make_stacks_transfer_mblock_only(sender: &StacksPrivateKey, nonce: u64, fee_rate: u64,
                                        recipient: &PrincipalData, amount: u64) -> Vec<u8> {
    let payload = TransactionPayload::TokenTransfer(recipient.clone(), amount, TokenTransferMemo([0; 34]));
    serialize_sign_standard_single_sig_tx_anchor_mode(payload.into(), sender, nonce, fee_rate, TransactionAnchorMode::OffChainOnly)
}

pub fn make_poison(sender: &StacksPrivateKey, nonce: u64, fee_rate: u64,
                   header_1: StacksMicroblockHeader, header_2: StacksMicroblockHeader) -> Vec<u8> {
    let payload = TransactionPayload::PoisonMicroblock(header_1, header_2);
    serialize_sign_standard_single_sig_tx(payload.into(), sender, nonce, fee_rate)
}

pub fn make_coinbase(sender: &StacksPrivateKey, nonce: u64, fee_rate: u64) -> Vec<u8> {
    let payload = TransactionPayload::Coinbase(CoinbasePayload([0; 32]));
    serialize_sign_standard_single_sig_tx(payload.into(), sender, nonce, fee_rate)
}

pub fn make_contract_call(
    sender: &StacksPrivateKey, nonce: u64, fee_rate: u64,
    contract_addr: &StacksAddress, contract_name: &str,
    function_name: &str, function_args: &[Value]) -> Vec<u8> {

    let contract_name = ContractName::from(contract_name);
    let function_name = ClarityName::from(function_name);

    let payload = TransactionContractCall {
        address: contract_addr.clone(),
        contract_name, function_name,
        function_args: function_args.iter().map(|x| x.clone()).collect()
    };

    serialize_sign_standard_single_sig_tx(payload.into(), sender, nonce, fee_rate)
}

fn make_microblock(privk: &StacksPrivateKey, chainstate: &mut StacksChainState, burn_block_hash: BurnchainHeaderHash, block: StacksBlock, block_cost: ExecutionCost, txs: Vec<StacksTransaction>) -> StacksMicroblock {
    let mut block_bytes = vec![];
    block.consensus_serialize(&mut block_bytes).unwrap();
    let block_size = block_bytes.len() as u64;

    let mut microblock_builder = StacksMicroblockBuilder::new(block.block_hash(), burn_block_hash.clone(), chainstate, block_cost, block_size).unwrap();
    let mempool_txs : Vec<_> = txs.into_iter()
        .map(|tx| {
            // TODO: better fee estimation
            let mut tx_bytes = vec![];
            tx.consensus_serialize(&mut tx_bytes).unwrap();
            let estimated_fee = (tx_bytes.len() as u64) * tx.get_fee_rate();
            MemPoolTxInfo::from_tx(tx, estimated_fee, burn_block_hash.clone(), block.block_hash(), block.header.total_work.work)
        })
        .collect();

    // NOTE: we intentionally do not check the block's microblock pubkey hash against the private
    // key, because we may need to test that microblocks get rejected due to bad signatures.
    let microblock = microblock_builder.mine_next_microblock_from_txs(mempool_txs, privk).unwrap();
    microblock
}

#[test]
fn should_succeed_mining_valid_txs() {
    let conf = new_test_conf();
    
    let num_rounds = 6;
    let mut run_loop = RunLoop::new(conf);

    // Use tenure's hook for submitting transactions
    run_loop.callbacks.on_new_tenure(|round, _burnchain_tip, chain_tip, tenure| {
        let header_hash = chain_tip.block.block_hash();
        let burn_header_hash = chain_tip.metadata.burn_header_hash;

        match round {
            1 => {
                // On round 1, publish the KV contract
                tenure.mem_pool.submit_raw(&burn_header_hash, &header_hash, PUBLISH_CONTRACT.to_owned()).unwrap();
            },
            2 => {
                // On round 2, publish a "get:foo" transaction
                // ./blockstack-cli --testnet contract-call 043ff5004e3d695060fa48ac94c96049b8c14ef441c50a184a6a3875d2a000f3 0 1 STGT7GSMZG7EA0TS6MVSKT5JC1DCDFGZWJJZXN8A store get-value -e \"foo\"
                let get_foo = "8080000000040021a3c334fc0ee50359353799e8b2605ac6be1fe40000000000000001000000000000000001007f9308b891b1593029c520cae33c25f55c4e720f875c85f8845e0ee7204047a0223f3587c033e0ddb7b0618183c56bf27a1521adf433d71f17d86a7b90c72973030200000000021a21a3c334fc0ee50359353799e8b2605ac6be1fe40573746f7265096765742d76616c7565000000010200000003666f6f";
                tenure.mem_pool.submit_raw(&burn_header_hash, &header_hash,hex_bytes(get_foo).unwrap().to_vec()).unwrap();
            },
            3 => {
                // On round 3, publish a "set:foo=bar" transaction
                // ./blockstack-cli --testnet contract-call 043ff5004e3d695060fa48ac94c96049b8c14ef441c50a184a6a3875d2a000f3 0 2 STGT7GSMZG7EA0TS6MVSKT5JC1DCDFGZWJJZXN8A store set-value -e \"foo\" -e \"bar\" 
                let set_foo_bar = "8080000000040021a3c334fc0ee50359353799e8b2605ac6be1fe400000000000000020000000000000000010132033d83ad5051a52cef15cb88a93ac046e91a7ea2c6bf2110efdf8827ad8e0c6d0fbce1087637647ecf771c16613637742c08a4422cddfe7af03227257061ad030200000000021a21a3c334fc0ee50359353799e8b2605ac6be1fe40573746f7265097365742d76616c7565000000020200000003666f6f0200000003626172";
                tenure.mem_pool.submit_raw(&burn_header_hash, &header_hash,hex_bytes(set_foo_bar).unwrap().to_vec()).unwrap();
            },
            4 => {
                // On round 4, publish a "get:foo" transaction
                // ./blockstack-cli --testnet contract-call 043ff5004e3d695060fa48ac94c96049b8c14ef441c50a184a6a3875d2a000f3 0 3 STGT7GSMZG7EA0TS6MVSKT5JC1DCDFGZWJJZXN8A store get-value -e \"foo\"
                let get_foo = "8080000000040021a3c334fc0ee50359353799e8b2605ac6be1fe4000000000000000300000000000000000100f1ffc472083f4fea947a6d1a83d0ddf0353dc0e9fac94d74da9d668b61676d1966474bc890f94c5fdb4d6ef816682f9073a2185e6ca8f8a6aa25a36ed851399d030200000000021a21a3c334fc0ee50359353799e8b2605ac6be1fe40573746f7265096765742d76616c7565000000010200000003666f6f";
                tenure.mem_pool.submit_raw(&burn_header_hash, &header_hash,hex_bytes(get_foo).unwrap().to_vec()).unwrap();
            },
            5 => {
                // On round 5, publish a stacks transaction
                // ./blockstack-cli --testnet token-transfer b1cf9cee5083f421c84d7cb53be5edf2801c3c78d63d53917aee0bdc8bd160ee01 0 0 ST195Q2HPXY576N4CT2A0R94D7DRYSX54A5X3YZTH 1000
                let transfer_1000_stx = "80800000000400b71a091b4b8b7661a661c620966ab6573bc2dcd3000000000000000000000000000000000000cf44fd240b404ec42a4e419ef2059add056980fed6f766e2f11e4b03a41afb885cfd50d2552ec3fff5c470d6975dfe4010cd17bef45e24e0c6e30c8ae6604b2f03020000000000051a525b8a36ef8a73548cd0940c248d3b71ecf4a45100000000000003e800000000000000000000000000000000000000000000000000000000000000000000";
                tenure.mem_pool.submit_raw(&burn_header_hash, &header_hash,hex_bytes(transfer_1000_stx).unwrap().to_vec()).unwrap();
            },
            _ => {}
        };
        return
    });

    // Use block's hook for asserting expectations
    run_loop.callbacks.on_new_stacks_chain_state(|round, _burnchain_tip, chain_tip, _chain_state| {
        match round {
            0 => {
                // Inspecting the chain at round 0.
                // - Chain length should be 1.
                assert!(chain_tip.metadata.block_height == 1);
                
                // Block #1 should only have 0 txs
                assert!(chain_tip.block.txs.len() == 1);

                // 0 event should have been produced
                let events: Vec<StacksTransactionEvent> = chain_tip.receipts.iter().flat_map(|a| a.events.clone()).collect();
                assert!(events.len() == 0);
            },
            1 => {
                // Inspecting the chain at round 1.
                // - Chain length should be 2.
                assert!(chain_tip.metadata.block_height == 2);
                
                // Block #2 should only have 2 txs
                assert!(chain_tip.block.txs.len() == 2);

                // Transaction #1 should be the coinbase from the leader
                let coinbase_tx = &chain_tip.block.txs[0];
                assert!(coinbase_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match coinbase_tx.payload {
                    TransactionPayload::Coinbase(_) => true,
                    _ => false,
                });

                // Transaction #2 should be the smart contract published
                let contract_tx = &chain_tip.block.txs[1];
                assert!(contract_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match contract_tx.payload {
                    TransactionPayload::SmartContract(_) => true,
                    _ => false,
                });

                // 0 event should have been produced
                let events: Vec<StacksTransactionEvent> = chain_tip.receipts.iter().flat_map(|a| a.events.clone()).collect();
                assert!(events.len() == 0);
            },
            2 => {
                // Inspecting the chain at round 2.
                // - Chain length should be 3.
                assert!(chain_tip.metadata.block_height == 3);
                
                // Block #3 should only have 2 txs
                assert!(chain_tip.block.txs.len() == 2);

                // Transaction #1 should be the coinbase from the leader
                let coinbase_tx = &chain_tip.block.txs[0];
                assert!(coinbase_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match coinbase_tx.payload {
                    TransactionPayload::Coinbase(_) => true,
                    _ => false,
                });

                // Transaction #2 should be the get-value contract-call
                let contract_tx = &chain_tip.block.txs[1];
                assert!(contract_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match contract_tx.payload {
                    TransactionPayload::ContractCall(_) => true,
                    _ => false,
                });

                // 0 event should have been produced
                let events: Vec<StacksTransactionEvent> = chain_tip.receipts.iter().flat_map(|a| a.events.clone()).collect();
                assert!(events.len() == 0);
            },
            3 => {
                // Inspecting the chain at round 3.
                // - Chain length should be 4.
                assert!(chain_tip.metadata.block_height == 4);
                
                // Block #4 should only have 2 txs
                assert!(chain_tip.block.txs.len() == 2);

                // Transaction #1 should be the coinbase from the leader
                let coinbase_tx = &chain_tip.block.txs[0];
                assert!(coinbase_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match coinbase_tx.payload {
                    TransactionPayload::Coinbase(_) => true,
                    _ => false,
                });

                // Transaction #2 should be the set-value contract-call
                let contract_tx = &chain_tip.block.txs[1];
                assert!(contract_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match contract_tx.payload {
                    TransactionPayload::ContractCall(_) => true,
                    _ => false,
                });
                
                // 1 event should have been produced
                let events: Vec<StacksTransactionEvent> = chain_tip.receipts.iter().flat_map(|a| a.events.clone()).collect();
                assert!(events.len() == 1);
                assert!(match &events[0] {
                    StacksTransactionEvent::SmartContractEvent(data) => {
                        format!("{}", data.key.0) == "STGT7GSMZG7EA0TS6MVSKT5JC1DCDFGZWJJZXN8A.store" &&
                        data.key.1 == "print" &&
                        format!("{}", data.value) == "0x53657474696e67206b657920666f6f" // "Setting key foo" in hexa
                    },
                    _ => false
                });
            },
            4 => {
                // Inspecting the chain at round 4.
                // - Chain length should be 5.
                assert!(chain_tip.metadata.block_height == 5);
                
                // Block #5 should only have 2 txs
                assert!(chain_tip.block.txs.len() == 2);

                // Transaction #1 should be the coinbase from the leader
                let coinbase_tx = &chain_tip.block.txs[0];
                assert!(coinbase_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match coinbase_tx.payload {
                    TransactionPayload::Coinbase(_) => true,
                    _ => false,
                });

                // Transaction #2 should be the get-value contract-call
                let contract_tx = &chain_tip.block.txs[1];
                assert!(contract_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match contract_tx.payload {
                    TransactionPayload::ContractCall(_) => true,
                    _ => false,
                });

                // 1 event should have been produced
                let events: Vec<StacksTransactionEvent> = chain_tip.receipts.iter().flat_map(|a| a.events.clone()).collect();
                assert!(events.len() == 1);
                assert!(match &events[0] {
                    StacksTransactionEvent::SmartContractEvent(data) => {
                        format!("{}", data.key.0) == "STGT7GSMZG7EA0TS6MVSKT5JC1DCDFGZWJJZXN8A.store" &&
                        data.key.1 == "print" &&
                        format!("{}", data.value) == "0x47657474696e67206b657920666f6f" // "Getting key foo" in hexa
                    },
                    _ => false
                });
            },
            5 => {
                // Inspecting the chain at round 5.
                // - Chain length should be 6.
                assert!(chain_tip.metadata.block_height == 6);
                
                // Block #6 should only have 2 txs
                assert!(chain_tip.block.txs.len() == 2);

                // Transaction #1 should be the coinbase from the leader
                let coinbase_tx = &chain_tip.block.txs[0];
                assert!(coinbase_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match coinbase_tx.payload {
                    TransactionPayload::Coinbase(_) => true,
                    _ => false,
                });

                // Transaction #2 should be the STX transfer
                let contract_tx = &chain_tip.block.txs[1];
                assert!(contract_tx.chain_id == TESTNET_CHAIN_ID);

                assert!(match contract_tx.payload {
                    TransactionPayload::TokenTransfer(_,_,_) => true,
                    _ => false,
                });

                // 1 event should have been produced
                let events: Vec<StacksTransactionEvent> = chain_tip.receipts.iter().flat_map(|a| a.events.clone()).collect();
                assert!(events.len() == 1);
                assert!(match &events[0] {
                    StacksTransactionEvent::STXEvent(STXEventType::STXTransferEvent(event)) => {
                        format!("{}", event.recipient) == "ST195Q2HPXY576N4CT2A0R94D7DRYSX54A5X3YZTH" &&
                        format!("{}", event.sender) == "ST2VHM28V9E5QCRD6C73215KAPSBKQGPWTEE5CMQT" &&                        
                        event.amount == 1000
                    },
                    _ => false
                });
            },
            _ => {}
        }
    });
    run_loop.start(num_rounds);
}

#[test]
#[ignore]
fn should_succeed_handling_malformed_and_valid_txs() {
    let conf = new_test_conf();
    
    let num_rounds = 4;
    let mut run_loop = RunLoop::new(conf);

    // Use tenure's hook for submitting transactions
    run_loop.callbacks.on_new_tenure(|round, _burnchain_tip, chain_tip, tenure| {
        let header_hash = chain_tip.block.block_hash();
        let burn_header_hash = chain_tip.metadata.burn_header_hash;

        match round {
            1 => {
                // On round 1, publish the KV contract
                let contract_sk = StacksPrivateKey::from_hex(SK_1).unwrap();
                let publish_contract = make_contract_publish(&contract_sk, 0, 0, "store", STORE_CONTRACT);
                tenure.mem_pool.submit_raw(&burn_header_hash, &header_hash,publish_contract).unwrap();
            },
            2 => {
                // On round 2, publish a "get:foo" transaction (mainnet instead of testnet).
                // Will not be mined
                // ./blockstack-cli contract-call 043ff5004e3d695060fa48ac94c96049b8c14ef441c50a184a6a3875d2a000f3 0 1 STGT7GSMZG7EA0TS6MVSKT5JC1DCDFGZWJJZXN8A store get-value -e \"foo\"
                let get_foo = "0000000000040021a3c334fc0ee50359353799e8b2605ac6be1fe4000000000000000100000000000000000100cbb46766a2bc03261f6bd428fdd6ce63da8ed04713e6476426390ccc15d2b1c133d9ba30a47b51cd467a09a25f3d7fa2bb4b85379f7d0601df02268cb623e231030200000000021a21a3c334fc0ee50359353799e8b2605ac6be1fe40573746f7265096765742d76616c7565000000010200000003666f6f";
                tenure.mem_pool.submit_raw(&burn_header_hash, &header_hash,hex_bytes(get_foo).unwrap().to_vec()).unwrap();
            },
            3 => {
                // On round 3, publish a "set:foo=bar" transaction (chain-id not matching).
                // Will not be mined
                // ./blockstack-cli --testnet contract-call 043ff5004e3d695060fa48ac94c96049b8c14ef441c50a184a6a3875d2a000f3 0 1 STGT7GSMZG7EA0TS6MVSKT5JC1DCDFGZWJJZXN8A store set-value -e \"foo\" -e \"bar\"
                let set_foo_bar = "8000000001040021a3c334fc0ee50359353799e8b2605ac6be1fe4000000000000000100000000000000000101e57846af212a3e9536c86446d3f39210f6edd691f5c6db65feea3e188822dc2c09e8f82b2f7449d54b58e1a6666b003f65c104f3f9b41a34211560b8ce2c1095030200000000021a21a3c334fc0ee50359353799e8b2605ac6be1fe40573746f7265097365742d76616c7565000000020200000003666f6f0200000003626172";
                tenure.mem_pool.submit_raw(&burn_header_hash, &header_hash,hex_bytes(set_foo_bar).unwrap().to_vec()).unwrap();
            },
            4 => {
                // On round 4, publish a "get:foo" transaction
                // ./blockstack-cli --testnet contract-call 043ff5004e3d695060fa48ac94c96049b8c14ef441c50a184a6a3875d2a000f3 0 1 STGT7GSMZG7EA0TS6MVSKT5JC1DCDFGZWJJZXN8A store get-value -e \"foo\"
                let get_foo = "8000000000040021a3c334fc0ee50359353799e8b2605ac6be1fe4000000000000000100000000000000000100e11fa0938e579c868137cfdd95fc0d6107a32c7a8864bbff2852c792c1759a38314e42922702b709c7b17c93d406f9d8057fb7c14736e5d85ff24acf89e921d6030200000000021a21a3c334fc0ee50359353799e8b2605ac6be1fe40573746f7265096765742d76616c7565000000010200000003666f6f";
                tenure.mem_pool.submit_raw(&burn_header_hash, &header_hash,hex_bytes(get_foo).unwrap().to_vec()).unwrap();
            },
            _ => {}
        };
        return
    });

    // Use block's hook for asserting expectations
    run_loop.callbacks.on_new_stacks_chain_state(|round, _burnchain_tip, chain_tip, _chain_state| {
        match round {
            0 => {
                // Inspecting the chain at round 0.
                // - Chain length should be 1.
                assert!(chain_tip.metadata.block_height == 1);
                
                // Block #1 should only have 1 txs
                assert!(chain_tip.block.txs.len() == 1);

                // Transaction #1 should be the coinbase from the leader
                let coinbase_tx = &chain_tip.block.txs[0];
                assert!(coinbase_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match coinbase_tx.payload {
                    TransactionPayload::Coinbase(_) => true,
                    _ => false,
                });
            },
            1 => {
                // Inspecting the chain at round 1.
                // - Chain length should be 2.
                assert!(chain_tip.metadata.block_height == 2);
                
                // Block #2 should only have 2 txs
                assert!(chain_tip.block.txs.len() == 2);

                // Transaction #1 should be the coinbase from the leader
                let coinbase_tx = &chain_tip.block.txs[0];
                assert!(coinbase_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match coinbase_tx.payload {
                    TransactionPayload::Coinbase(_) => true,
                    _ => false,
                });

                // Transaction #2 should be the smart contract published
                let contract_tx = &chain_tip.block.txs[1];
                assert!(contract_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match contract_tx.payload {
                    TransactionPayload::SmartContract(_) => true,
                    _ => false,
                });
            },
            2 => {
                // Inspecting the chain at round 2.
                // - Chain length should be 3.
                assert!(chain_tip.metadata.block_height == 3);
                
                // Block #3 should only have 1 tx (the other being invalid)
                assert!(chain_tip.block.txs.len() == 1);

                // Transaction #1 should be the coinbase from the leader
                let coinbase_tx = &chain_tip.block.txs[0];
                assert!(coinbase_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match coinbase_tx.payload {
                    TransactionPayload::Coinbase(_) => true,
                    _ => false,
                });
            },
            3 => {
                // Inspecting the chain at round 3.
                // - Chain length should be 4.
                assert!(chain_tip.metadata.block_height == 4);
                
                // Block #4 should only have 1 tx (the other being invalid)
                assert!(chain_tip.block.txs.len() == 1);

                // Transaction #1 should be the coinbase from the leader
                let coinbase_tx = &chain_tip.block.txs[0];
                assert!(coinbase_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match coinbase_tx.payload {
                    TransactionPayload::Coinbase(_) => true,
                    _ => false,
                });
            },
            4 => {
                // Inspecting the chain at round 4.
                // - Chain length should be 5.
                assert!(chain_tip.metadata.block_height == 5);
                
                // Block #5 should only have 2 txs
                assert!(chain_tip.block.txs.len() == 2);

                // Transaction #1 should be the coinbase from the leader
                let coinbase_tx = &chain_tip.block.txs[0];
                assert!(coinbase_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match coinbase_tx.payload {
                    TransactionPayload::Coinbase(_) => true,
                    _ => false,
                });

                // Transaction #2 should be the contract-call 
                let contract_tx = &chain_tip.block.txs[1];
                assert!(contract_tx.chain_id == TESTNET_CHAIN_ID);
                assert!(match contract_tx.payload {
                    TransactionPayload::ContractCall(_) => true,
                    _ => false,
                });
            },
            _ => {}
        }
    });
    run_loop.start(num_rounds);
}

#[test]
fn test_btc_to_sat() {
    let inputs = [
        "0.10000000",
        "0.00000010",
        "0.00000001",
        "1.00000001",
        "0.1",
        "0.00000000",
        "0.00001192",
    ];
    let expected_outputs: [u64; 7] = [
        10000000,
        10,
        1,
        100000001,
        10000000,
        0,
        1192
    ];

    for (input, expected_output) in inputs.iter().zip(expected_outputs.iter()) {
        let output = ParsedUTXO::serialized_btc_to_sat(input).unwrap(); 
        assert_eq!(*expected_output, output);
    }
}

#[test]
fn test_btc_to_sat_errors() {
    assert!(ParsedUTXO::serialized_btc_to_sat("0.000000001").is_none());
    assert!(ParsedUTXO::serialized_btc_to_sat("1").is_none());
    assert!(ParsedUTXO::serialized_btc_to_sat("1e-8").is_none());
    assert!(ParsedUTXO::serialized_btc_to_sat("7.4e-7").is_none());
    assert!(ParsedUTXO::serialized_btc_to_sat("5.96e-6").is_none());
}
