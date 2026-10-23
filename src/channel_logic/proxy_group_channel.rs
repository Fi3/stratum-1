//! # Proxy Group Channel
//!
//! This module contains logic for managing Standard Channels via Group Channels.

use crate::{common_properties::StandardChannel, parsers::MiningOwned, Error};
use mining_sv2::{
    NewExtendedMiningJob, NewExtendedMiningJobOwned, NewMiningJobOwned,
    OpenStandardMiningChannelSuccess, SetNewPrevHashOwned,
};

use super::extended_to_standard_job;
use nohash_hasher::BuildNoHashHasher;
use std::collections::HashMap;

/// Wrapper around `GroupChannel` for managing multiple group channels
#[derive(Debug, Clone, Default)]
pub struct GroupChannels {
    channels: HashMap<u32, GroupChannel>,
}
impl GroupChannels {
    /// Constructor
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
        }
    }
    /// Called when when a group channel created. We add the channel in its
    /// respective group and call `GroupChannel::on_channel_success_for_hom_downtream`
    pub fn on_channel_success_for_hom_downtream(
        &mut self,
        m: &OpenStandardMiningChannelSuccess,
    ) -> Result<Vec<MiningOwned>, Error> {
        let group_id = m.group_channel_id;

        self.channels
            .entry(group_id)
            .or_insert_with(GroupChannel::new);
        match self.channels.get_mut(&group_id) {
            Some(group) => group.on_channel_success_for_hom_downtream(m.clone()),
            None => unreachable!(),
        }
    }
    /// Called when a new prev hash arrives. We loop through all group channels to update state
    /// within each group
    pub fn update_new_prev_hash(&mut self, m: &SetNewPrevHashOwned) {
        if let Some(group) = self.channels.get_mut(&m.channel_id) {
            group.update_new_prev_hash(m);
        }
    }
    /// Cache work only in the group addressed by its native channel ID.
    pub fn on_new_extended_mining_job(&mut self, m: &NewExtendedMiningJob) {
        if let Some(group) = self.channels.get_mut(&m.channel_id) {
            group.on_new_extended_mining_job(m.as_owned());
        }
    }

    /// Move registered standard channels to their newly assigned group.
    pub fn set_group_channel(
        &mut self,
        message: &mining_sv2::SetGroupChannel<'_>,
    ) -> Result<(), Error> {
        if message.channel_ids.iter().any(|id| {
            !self
                .channels
                .values()
                .any(|g| g.hom_downstreams.contains_key(id))
        }) {
            return Err(Error::NotFoundChannelId);
        }
        for id in message.channel_ids.iter() {
            let old = self
                .channels
                .iter()
                .find_map(|(group_id, group)| {
                    group.hom_downstreams.contains_key(id).then_some(*group_id)
                })
                .ok_or(Error::NotFoundChannelId)?;
            let mut channel = self
                .channels
                .get_mut(&old)
                .and_then(|g| g.hom_downstreams.remove(id))
                .ok_or(Error::NotFoundChannelId)?;
            channel.group_id = message.group_channel_id;
            self.channels
                .entry(message.group_channel_id)
                .or_insert_with(GroupChannel::new)
                .hom_downstreams
                .insert(*id, channel);
        }
        Ok(())
    }

    /// Returns last valid job as a `NewMiningJob`
    pub fn last_received_job_to_standard_job(
        &mut self,
        channel_id: u32,
        group_id: u32,
    ) -> Result<NewMiningJobOwned, Error> {
        match self.channels.get_mut(&group_id) {
            Some(group) => group.last_received_job_to_standard_job(channel_id),
            None => Err(Error::GroupIdNotFound),
        }
    }

    /// Get group channel ids
    pub fn ids(&self) -> Vec<u32> {
        self.channels.keys().copied().collect()
    }
}

#[derive(Debug, Clone)]
struct GroupChannel {
    hom_downstreams: HashMap<u32, StandardChannel, BuildNoHashHasher<u32>>,
    future_jobs: Vec<NewExtendedMiningJobOwned>,
    last_prev_hash: Option<SetNewPrevHashOwned>,
    last_valid_job: Option<NewExtendedMiningJobOwned>,
    last_received_job: Option<NewExtendedMiningJobOwned>,
}

impl GroupChannel {
    fn new() -> Self {
        Self {
            hom_downstreams: HashMap::with_hasher(BuildNoHashHasher::default()),
            future_jobs: vec![],
            last_prev_hash: None,
            last_valid_job: None,
            last_received_job: None,
        }
    }
    // Called when a channel is successfully opened for header only mining(HOM) on standard
    // channels. Here, we store the new channel, and update state for jobs and return relevant
    // SV2 messages (NewMiningJob and SNPH)
    fn on_channel_success_for_hom_downtream(
        &mut self,
        m: OpenStandardMiningChannelSuccess,
    ) -> Result<Vec<MiningOwned>, Error> {
        let channel = StandardChannel {
            channel_id: m.channel_id,
            group_id: m.group_channel_id,
            target: m.target.into(),
            extranonce: m.extranonce_prefix.into(),
        };
        let channel_id = m.channel_id;
        let mut res = vec![];
        for extended_job in &self.future_jobs {
            let standard_job = extended_to_standard_job(
                extended_job,
                channel.extranonce.as_ref(),
                channel.channel_id,
                None,
                None,
            )
            .ok_or(Error::ImpossibleToCalculateMerkleRoot)?;
            res.push(MiningOwned::NewMiningJob(standard_job));
        }

        if let Some(last_valid_job) = &self.last_valid_job {
            let mut standard_job = extended_to_standard_job(
                last_valid_job,
                channel.extranonce.as_ref(),
                channel.channel_id,
                None,
                None,
            )
            .ok_or(Error::ImpossibleToCalculateMerkleRoot)?;

            if let Some(new_prev_hash) = &self.last_prev_hash {
                let mut new_prev_hash = new_prev_hash.clone();
                standard_job.set_future();
                new_prev_hash.job_id = standard_job.job_id;
                res.push(MiningOwned::NewMiningJob(standard_job));
                res.push(MiningOwned::SetNewPrevHash(new_prev_hash))
            } else {
                res.push(MiningOwned::NewMiningJob(standard_job));
            }
        } else if let Some(new_prev_hash) = &self.last_prev_hash {
            res.push(MiningOwned::SetNewPrevHash(new_prev_hash.clone()))
        }

        self.hom_downstreams.insert(channel_id, channel);

        Ok(res)
    }

    // If a matching job is already in the future job queue,
    // we set a new valid job, otherwise we clear the future jobs
    // queue and stage a prev hash to be used when the job arrives
    fn update_new_prev_hash(&mut self, m: &SetNewPrevHashOwned) {
        while let Some(job) = self.future_jobs.pop() {
            if job.job_id == m.job_id {
                self.last_valid_job = Some(job);
                break;
            }
        }
        self.future_jobs = vec![];
        let cloned = SetNewPrevHashOwned {
            channel_id: m.channel_id,
            job_id: m.job_id,
            prev_hash: m.prev_hash.clone(),
            min_ntime: m.min_ntime,
            nbits: m.nbits,
        };
        self.last_prev_hash = Some(cloned.clone());
    }

    // Pushes new job to future_job queue if it is future,
    // otherwise we set it as the valid job
    fn on_new_extended_mining_job(&mut self, m: NewExtendedMiningJobOwned) {
        self.last_received_job = Some(m.clone());
        if m.is_future() {
            self.future_jobs.push(m)
        } else {
            self.last_valid_job = Some(m)
        }
    }

    // Returns most recent job
    fn last_received_job_to_standard_job(
        &mut self,
        channel_id: u32,
    ) -> Result<NewMiningJobOwned, Error> {
        match &self.last_received_job {
            Some(m) => {
                let downstream = self
                    .hom_downstreams
                    .get(&channel_id)
                    .ok_or(Error::NotFoundChannelId)?;
                extended_to_standard_job(
                    m,
                    downstream.extranonce.as_ref(),
                    downstream.channel_id,
                    None,
                    None,
                )
                .ok_or(Error::ImpossibleToCalculateMerkleRoot)
            }
            None => Err(Error::NoValidJob),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use binary_sv2::B064KOwned;
    use std::convert::TryFrom;

    #[test]
    fn group_channel_new_prev_hash_ordering_test() {
        let mut group_channel = GroupChannel::new();
        let mut new_extended_mining_job = NewExtendedMiningJobOwned {
            channel_id: 1,
            job_id: 0,
            min_ntime: binary_sv2::Sv2OptionOwned::new(None),
            version: 0,
            version_rolling_allowed: false,
            merkle_path: vec![].try_into().unwrap(),
            coinbase_tx_prefix: B064KOwned::try_from(Vec::new()).unwrap(),
            coinbase_tx_suffix: B064KOwned::try_from(Vec::new()).unwrap(),
        };

        group_channel.on_new_extended_mining_job(new_extended_mining_job.clone());
        new_extended_mining_job.version = 1;
        group_channel.on_new_extended_mining_job(new_extended_mining_job);

        // Make sure this returns the last job - the one where we updated the version.
        group_channel.update_new_prev_hash(&SetNewPrevHashOwned {
            channel_id: 1,
            job_id: 0,
            prev_hash: [
                3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
                3, 3, 3, 3,
            ]
            .into(),
            min_ntime: 989898,
            nbits: 9,
        });

        assert_eq!(group_channel.last_valid_job.unwrap().version, 1);
    }
}
