use std::option::Option;
use std::sync::Arc;

use rocksdb::DB;
use rocksdb::rocksdb::Snapshot;

use proto::metapb;
use proto::raft_serverpb::RaftTruncatedState;
use raftserver::{Result, other};
use super::keys;
use super::engine::Retriever;

pub const RAFT_INIT_LOG_TERM: u64 = 5;
pub const RAFT_INIT_LOG_INDEX: u64 = 10;

pub struct PeerMeta {
    engine: Arc<DB>,

    pub region_id: u64,
    pub region: metapb::Region,
    pub last_index: u64,
    pub applied_index: u64,
    pub truncated_state: Option<RaftTruncatedState>,
}

impl PeerMeta {
    pub fn is_initialized(&self) -> bool {
        self.region.get_end_key().len() > 0
    }

    pub fn get_truncated_state(&self) -> Result<RaftTruncatedState> {
        match self.truncated_state {
            None => return Err(other("un-initialized truncated state")),
            Some(ref state) => return Ok(state.clone()),
        }
    }

    pub fn get_first_index(&self) -> Result<u64> {
        self.get_truncated_state().map(|state| state.get_index() + 1)
    }


    pub fn load_truncated_state(&self) -> Result<RaftTruncatedState> {
        let res: Option<RaftTruncatedState> = try!(self.engine.get_msg(
                                         &keys::raft_truncated_state_key(self.region_id)));

        if let Some(state) = res {
            return Ok(state);
        }

        let mut state = RaftTruncatedState::new();

        if self.is_initialized() {
            state.set_index(RAFT_INIT_LOG_INDEX);
            state.set_term(RAFT_INIT_LOG_TERM);
        } else {
            state.set_index(0);
            state.set_term(0);
        }

        Ok(state)
    }

    pub fn load_last_index(&self) -> Result<u64> {
        let n = try!(self.engine.get_u64(&keys::raft_last_index_key(self.region_id)));
        match n {
            Some(last_index) => return Ok(last_index),
            None => {
                // Log is empty, maybe we starts from scratch or have truncated all logs.
                let state = try!(self.get_truncated_state());
                return Ok(state.get_index());
            }
        }
    }

    pub fn load_applied_index<T: Retriever>(&self, db: &T) -> Result<u64> {
        let mut applied_index: u64 = 0;
        if self.is_initialized() {
            applied_index = RAFT_INIT_LOG_INDEX;
        }

        let n = try!(db.get_u64(&keys::raft_applied_index_key(self.region_id)));
        Ok(n.unwrap_or(applied_index))
    }

    // For region snapshot, we care 3 range in database for this region.
    // [region id, region id + 1) -> saving raft entries, applied index, etc.
    // [region meta start, region meta end) -> saving region information.
    // [region data start, region data end) -> saving region data.
    fn region_key_ranges(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        // The first range starts at MIN_KEY, but it contains unnecessary local data.
        // So we should skip this.
        let mut data_start_key = self.region.get_start_key();
        if data_start_key == keys::MIN_KEY {
            data_start_key = keys::LOCAL_MAX;
        }

        vec![(keys::region_id_prefix(self.region_id),
              keys::region_id_prefix(self.region_id + 1)),
             (keys::region_meta_prefix(self.region.get_start_key()),
              keys::region_meta_prefix(self.region.get_end_key())),
             (data_start_key.to_vec(), self.region.get_end_key().to_vec())]

    }

    pub fn snap_scan_region<F>(&self, snap: &Snapshot, f: &mut F) -> Result<()>
        where F: FnMut(&[u8], &[u8]) -> Result<bool>
    {
        let ranges = self.region_key_ranges();
        for r in ranges {
            try!(snap.scan(&r.0, &r.1, f));
        }

        Ok(())
    }
}