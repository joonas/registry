use std::{collections::HashMap, sync::Arc};
use tokio::{
    sync::{mpsc::Receiver, RwLock},
    task::JoinHandle,
};
use warg_crypto::hash::{Hash, Sha256};
use warg_protocol::registry::LogLeaf;
use warg_transparency::log::{LogBuilder, LogData, LogProofBundle, Node, VecLog};

use super::DataServiceError;

pub type ProofLog = VecLog<Sha256, LogLeaf>;

pub struct Input {
    pub data: ProofData,
    pub log_rx: Receiver<LogLeaf>,
}

pub struct Output {
    pub data: Arc<RwLock<ProofData>>,
    pub handle: JoinHandle<()>,
}

#[derive(Default)]
pub struct ProofData {
    log: ProofLog,
    leaf_index: HashMap<LogLeaf, Node>,
    root_index: HashMap<Hash<Sha256>, usize>,
}

impl ProofData {
    pub fn new(init: LogLeaf) -> Self {
        let mut log = ProofLog::default();
        let init_node = log.push(&init);

        let mut leaf_index = HashMap::new();
        let mut root_index = HashMap::new();

        let checkpoint = log.checkpoint();
        leaf_index.insert(init, init_node);
        root_index.insert(checkpoint.root(), checkpoint.length());

        Self {
            log,
            leaf_index,
            root_index,
        }
    }

    /// Generate a proof bundle for the consistency of the log across two times
    pub fn consistency(
        &self,
        old_root: &Hash<Sha256>,
        new_root: &Hash<Sha256>,
    ) -> Result<LogProofBundle<Sha256, LogLeaf>, DataServiceError> {
        let old_len = self
            .root_index
            .get(old_root)
            .ok_or_else(|| DataServiceError::RootNotFound(old_root.clone()))?;
        let new_len = self
            .root_index
            .get(new_root)
            .ok_or_else(|| DataServiceError::RootNotFound(new_root.clone()))?;

        let proof = self.log.prove_consistency(*old_len, *new_len);
        let bundle = LogProofBundle::bundle(vec![proof], vec![], &self.log)
            .map_err(DataServiceError::BundleFailure)?;
        Ok(bundle)
    }

    /// Generate a proof bundle for a group of inclusion proofs
    pub fn inclusion(
        &self,
        root: &Hash<Sha256>,
        leaves: &[LogLeaf],
    ) -> Result<LogProofBundle<Sha256, LogLeaf>, DataServiceError> {
        let log_length = *self
            .root_index
            .get(root)
            .ok_or_else(|| DataServiceError::RootNotFound(root.clone()))?;
        let mut proofs = Vec::new();
        for leaf in leaves {
            let node = *self
                .leaf_index
                .get(leaf)
                .ok_or_else(|| DataServiceError::LeafNotFound(leaf.clone()))?;
            let proof = self.log.prove_inclusion(node, log_length);
            proofs.push(proof);
        }
        let bundle = LogProofBundle::bundle(vec![], proofs, &self.log)
            .map_err(DataServiceError::BundleFailure)?;
        Ok(bundle)
    }
}

pub fn process(input: Input) -> Output {
    let Input { data, mut log_rx } = input;
    let data = Arc::new(RwLock::new(data));
    let processor_data = data.clone();

    let handle = tokio::spawn(async move {
        let data = processor_data;

        while let Some(leaf) = log_rx.recv().await {
            let mut data = data.as_ref().write().await;
            let node = data.log.push(&leaf);

            let checkpoint = data.log.checkpoint();
            data.root_index
                .insert(checkpoint.root(), checkpoint.length());
            data.leaf_index.insert(leaf, node);

            drop(data);
        }
    });

    Output { data, handle }
}