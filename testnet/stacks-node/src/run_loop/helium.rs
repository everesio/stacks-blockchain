use crate::{Config, Node, BurnchainController, MocknetController, BitcoinRegtestController, ChainTip};

use stacks::chainstate::stacks::db::ClarityTx;

use super::RunLoopCallbacks;

/// RunLoop is coordinating a simulated burnchain and some simulated nodes
/// taking turns in producing blocks.
pub struct RunLoop {
    config: Config,
    pub node: Node,
    pub callbacks: RunLoopCallbacks,
}

impl RunLoop {
    pub fn new(config: Config) -> Self {
        RunLoop::new_with_boot_exec(config, |_| {})
    }

    /// Sets up a runloop and node, given a config.
    pub fn new_with_boot_exec<F>(config: Config, boot_exec: F) -> Self
    where F: Fn(&mut ClarityTx) -> () {

        // Build node based on config
        let node = Node::new(config.clone(), boot_exec);

        Self {
            config,
            node,
            callbacks: RunLoopCallbacks::new(),
        }
    }

    /// Starts the testnet runloop.
    /// 
    /// This function will block by looping infinitely.
    /// It will start the burnchain (separate thread), set-up a channel in
    /// charge of coordinating the new blocks coming from the burnchain and 
    /// the nodes, taking turns on tenures.  
    pub fn start(&mut self, expected_num_rounds: u64) {

        // Initialize and start the burnchain.
        let mut burnchain: Box<dyn BurnchainController> = match &self.config.burnchain.mode[..] {
            "helium" => {
                BitcoinRegtestController::generic(self.config.clone())
            },
            "mocknet" => {
                MocknetController::generic(self.config.clone())
            }
            _ => unreachable!()
        };

        self.callbacks.invoke_burn_chain_initialized(&mut burnchain);

        let mut burnchain_tip = burnchain.start();

        // Update each node with the genesis block.
        self.node.process_burnchain_state(&burnchain_tip);

        // make first non-genesis block, with initial VRF keys
        self.node.setup(&mut burnchain);

        // Waiting on the 1st block (post-genesis) from the burnchain, containing the first key registrations 
        // that will be used for bootstraping the chain.
        let mut round_index: u64 = 0;

        let (mut artifacts, mut chain_tip) = match burnchain_tip.block_snapshot.block_height == 0 {
            true => {
                info!("Initiating new chain");
                // Sync and update node with this new block.
                burnchain_tip = burnchain.sync();
                self.node.process_burnchain_state(&burnchain_tip); // todo(ludo): should return genesis?
                let chain_tip = ChainTip::genesis();

                // Bootstrap the chain: node will start a new tenure,
                // using the sortition hash from block #1 for generating a VRF.
                let mut first_tenure = match self.node.initiate_genesis_tenure(&burnchain_tip) {
                    Some(res) => res,
                    None => panic!("Error while initiating genesis tenure")
                };

                self.callbacks.invoke_new_tenure(round_index, &burnchain_tip, &chain_tip, &mut first_tenure);

                // Run the tenure, keep the artifacts
                let artifacts_from_1st_tenure = match first_tenure.run() {
                    Some(res) => res,
                    None => panic!("Error while running 1st tenure")
                };

                // Tenures are instantiating their own chainstate, so that nodes can keep a clean chainstate,
                // while having the option of running multiple tenures concurrently and try different strategies.
                // As a result, once the tenure ran and we have the artifacts (anchored_blocks, microblocks),
                // we have the 1st node (leading) updating its chainstate with the artifacts from its own tenure.
                self.node.commit_artifacts(
                    &artifacts_from_1st_tenure.anchored_block, 
                    &artifacts_from_1st_tenure.parent_block, 
                    &mut burnchain, 
                    artifacts_from_1st_tenure.burn_fee);

                burnchain_tip = burnchain.sync();
                self.callbacks.invoke_new_burn_chain_state(round_index, &burnchain_tip, &chain_tip);
                

                let (last_sortitioned_block, won_sortition) = match self.node.process_burnchain_state(&burnchain_tip) {
                    (Some(sortitioned_block), won_sortition) => (sortitioned_block, won_sortition),
                    (None, _) => panic!("Node should have a sortitioned block")
                };
                
                // Have the node process its own tenure.
                // We should have some additional checks here, and ensure that the previous artifacts are legit.
        
                let chain_tip = self.node.process_tenure(
                    &artifacts_from_1st_tenure.anchored_block, 
                    &last_sortitioned_block.block_snapshot.burn_header_hash, 
                    &last_sortitioned_block.block_snapshot.parent_burn_header_hash, 
                    artifacts_from_1st_tenure.microblocks.clone(),
                    burnchain.burndb_mut());
        
                self.callbacks.invoke_new_stacks_chain_state(
                    round_index, 
                    &burnchain_tip, 
                    &chain_tip, 
                    &mut self.node.chain_state);
        
                // If the node we're looping on won the sortition, initialize and configure the next tenure
                if !won_sortition {
                    panic!("")
                }

                (Some(artifacts_from_1st_tenure), chain_tip)
            },
            false => {
                info!("Loading initiated chain data from path {}", self.config.node.working_dir);

                let last_burnchain_block_processed = self.node.restore_chainstate(&burnchain_tip); // todo(ludo): should return genesis?

                if !last_burnchain_block_processed {
                    self.node.process_burnchain_state(&burnchain_tip); // todo(ludo): should return genesis?
                }
                // todo(ludo): chain_tip should be restored first.
                let mut first_tenure = match self.node.initiate_new_tenure() {
                    Some(res) => res,
                    None => panic!("Error while initiating genesis tenure")
                };

                // Run the tenure, keep the artifacts
                let artifacts_from_1st_tenure = match first_tenure.run() {
                    Some(res) => res,
                    None => panic!("Error while running 1st tenure")
                };

                // Tenures are instantiating their own chainstate, so that nodes can keep a clean chainstate,
                // while having the option of running multiple tenures concurrently and try different strategies.
                // As a result, once the tenure ran and we have the artifacts (anchored_blocks, microblocks),
                // we have the 1st node (leading) updating its chainstate with the artifacts from its own tenure.
                self.node.commit_artifacts(
                    &artifacts_from_1st_tenure.anchored_block, 
                    &artifacts_from_1st_tenure.parent_block, 
                    &mut burnchain, 
                    artifacts_from_1st_tenure.burn_fee);

                burnchain_tip = burnchain.sync();                

                let (last_sortitioned_block, won_sortition) = match self.node.process_burnchain_state(&burnchain_tip) {
                    (Some(sortitioned_block), won_sortition) => (sortitioned_block, won_sortition),
                    (None, _) => panic!("Node should have a sortitioned block")
                };
                
                // Have the node process its own tenure.
                // We should have some additional checks here, and ensure that the previous artifacts are legit.
        
                let chain_tip = self.node.process_tenure(
                    &artifacts_from_1st_tenure.anchored_block, 
                    &last_sortitioned_block.block_snapshot.burn_header_hash, 
                    &last_sortitioned_block.block_snapshot.parent_burn_header_hash, 
                    artifacts_from_1st_tenure.microblocks.clone(),
                    burnchain.burndb_mut());
        
                self.callbacks.invoke_new_stacks_chain_state(
                    round_index, 
                    &burnchain_tip, 
                    &chain_tip, 
                    &mut self.node.chain_state);
        
                // If the node we're looping on won the sortition, initialize and configure the next tenure
                if !won_sortition {
                    panic!("")
                }
        
                (Some(artifacts_from_1st_tenure), chain_tip)
            }
        };
        
        self.node.spawn_peer_server();

        let mut leader_tenure = self.node.initiate_new_tenure();

        // Start the runloop
        round_index = 1;
        loop {
            if expected_num_rounds == round_index {
                return;
            }

            // Run the last initialized tenure
            let artifacts_from_tenure = match leader_tenure {
                Some(mut tenure) => {
                    self.callbacks.invoke_new_tenure(round_index, &burnchain_tip, &chain_tip, &mut tenure);
                    tenure.run()
                },
                None => None
            };

            match artifacts_from_tenure {
                Some(ref artifacts) => {
                    // Have each node receive artifacts from the current tenure
                    self.node.commit_artifacts(
                        &artifacts.anchored_block, 
                        &artifacts.parent_block, 
                        &mut burnchain, 
                        artifacts.burn_fee);
                },
                None => {}
            }

            burnchain_tip = burnchain.sync();
            self.callbacks.invoke_new_burn_chain_state(round_index, &burnchain_tip, &chain_tip);
    
            leader_tenure = None;

            // todo(ludo): A panic happening now will be an issue

            // Have each node process the new block, that can include, or not, a sortition.
            let (last_sortitioned_block, won_sortition) = match self.node.process_burnchain_state(&burnchain_tip) {
                (Some(sortitioned_block), won_sortition) => (sortitioned_block, won_sortition),
                (None, _) => panic!("Node should have a sortitioned block")
            };

            if round_index  == 3 {
                panic!("Let's panic");
            }

            match artifacts_from_tenure {
                // Pass if we're missing the artifacts from the current tenure.
                None => continue,
                Some(ref artifacts) => {
                    // Have the node process its tenure.
                    // We should have some additional checks here, and ensure that the previous artifacts are legit.
                    chain_tip = self.node.process_tenure(
                        &artifacts.anchored_block, 
                        &last_sortitioned_block.block_snapshot.burn_header_hash, 
                        &last_sortitioned_block.block_snapshot.parent_burn_header_hash,             
                        artifacts.microblocks.clone(),
                        burnchain.burndb_mut());

                        self.callbacks.invoke_new_stacks_chain_state(
                            round_index, 
                            &burnchain_tip, 
                            &chain_tip, 
                            &mut self.node.chain_state);
                },
            };
            
            // If won sortition, initialize and configure the next tenure
            if won_sortition {
                leader_tenure = self.node.initiate_new_tenure();
            } 
            
            round_index += 1;
        }
    }
}
