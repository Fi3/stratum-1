//! # Channel Factory
//!
//! This module contains logic for creating and managing channels.

use super::channel_coinbase_output::append_coinbase_output;
pub use super::channel_coinbase_output::ChannelCoinbaseOutput;
use super::extended_to_standard_job;
use crate::mining::{ExtendedExtranonce, Target};
use crate::{
    common_properties::StandardChannel,
    job_creator::{self, JobsCreators},
    parsers::MiningOwned,
    utils::{GroupId, Id, Mutex},
    Error,
};
use mining_sv2::{
    NewExtendedMiningJobOwned, NewMiningJobOwned, OpenExtendedMiningChannelSuccessOwned,
    OpenMiningChannelError, OpenStandardMiningChannelSuccessOwned, SetCustomMiningJobErrorOwned,
    SetCustomMiningJobOwned, SetCustomMiningJobSuccess, SetExtranoncePrefixOwned,
    SetNewPrevHashOwned, SubmitSharesErrorOwned, SubmitSharesExtendedOwned, SubmitSharesStandard,
};
use template_distribution_sv2::{
    NewTemplateOwned, SetNewPrevHashOwned as SetNewPrevHashFromTpOwned,
};

use hex::DisplayHex;
use nohash_hasher::BuildNoHashHasher;
use rand::{rngs::OsRng, RngCore};
use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
    sync::Arc,
};

use tracing::{debug, error, info, trace, warn};

use bitcoin::{
    block::{Header, Version},
    hash_types,
    hashes::sha256d::Hash,
    CompactTarget, TxOut,
};

/// A stripped type of `SetCustomMiningJob` without the (`channel_id, `request_id` and `token`)
/// fields
#[derive(Debug)]
pub struct PartialSetCustomMiningJob {
    pub version: u32,
    pub prev_hash: binary_sv2::U256Owned,
    pub min_ntime: u32,
    pub nbits: u32,
    pub coinbase_tx_version: u32,
    pub coinbase_prefix: binary_sv2::B0255Owned,
    pub coinbase_tx_input_n_sequence: u32,
    pub coinbase_tx_outputs: binary_sv2::B064KOwned,
    pub coinbase_tx_locktime: u32,
    pub merkle_path: binary_sv2::Seq0255Owned<binary_sv2::U256Owned>,
    pub future_job: bool,
}

/// Represents the action that needs to be done when a new share is received.
#[derive(Debug, Clone)]
pub enum OnNewShare {
    /// Used when the received is malformed, is for an inexistent channel or do not meet downstream
    /// target.
    SendErrorDownstream(SubmitSharesErrorOwned),
    /// Used when an extended channel in a proxy receive a share, and the share meet upstream
    /// target, in this case a new share must be sent upstream. Also an optional template id is
    /// returned, when a job declarator want to send a valid share upstream could use the
    /// template for get the up job id.
    SendSubmitShareUpstream((Share, Option<u64>)),
    /// Used when a group channel in a proxy receive a share that is not malformed and is for a
    /// valid channel in that case we relay the same exact share upstream with a new request id.
    RelaySubmitShareUpstream,
    /// Indicate that the share meet bitcoin target, when there is an upstream the we should send
    /// the share upstream, whenever possible we should also notify the TP about it.
    /// When a pool negotiate a job with downstream we do not have the template_id so we set it to
    /// None
    /// (share, template id, coinbase,complete extranonce)
    ShareMeetBitcoinTarget((Share, Option<u64>, Vec<u8>, Vec<u8>)),
    /// Indicate that the share meet downstream target, in the case we could send a success
    /// response downstream.
    ShareMeetDownstreamTarget,
}

impl OnNewShare {
    /// Converts standard share into extended share
    pub fn into_extended(&mut self, extranonce: Vec<u8>, up_id: u32) {
        match self {
            OnNewShare::SendErrorDownstream(_) => (),
            OnNewShare::SendSubmitShareUpstream((share, template_id)) => match share {
                Share::Extended(_) => (),
                Share::Standard((share, _)) => {
                    let share = SubmitSharesExtendedOwned {
                        channel_id: up_id,
                        sequence_number: share.sequence_number,
                        job_id: share.job_id,
                        nonce: share.nonce,
                        ntime: share.ntime,
                        version: share.version,
                        extranonce: extranonce.try_into().unwrap(),
                    };
                    *self = Self::SendSubmitShareUpstream((Share::Extended(share), *template_id));
                }
            },
            OnNewShare::RelaySubmitShareUpstream => (),
            OnNewShare::ShareMeetBitcoinTarget((share, t_id, coinbase, ext)) => match share {
                Share::Extended(_) => (),
                Share::Standard((share, _)) => {
                    let share = SubmitSharesExtendedOwned {
                        channel_id: up_id,
                        sequence_number: share.sequence_number,
                        job_id: share.job_id,
                        nonce: share.nonce,
                        ntime: share.ntime,
                        version: share.version,
                        extranonce: extranonce.try_into().unwrap(),
                    };
                    *self = Self::ShareMeetBitcoinTarget((
                        Share::Extended(share),
                        *t_id,
                        coinbase.clone(),
                        ext.to_vec(),
                    ));
                }
            },
            OnNewShare::ShareMeetDownstreamTarget => todo!(),
        }
    }
}

/// A share can be either extended or standard
#[derive(Clone, Debug)]
pub enum Share {
    Extended(SubmitSharesExtendedOwned),
    // share, group id
    Standard((SubmitSharesStandard, u32)),
}

/// Helper type used before a `SetNewPrevHash` has a channel_id
#[derive(Clone, Debug)]
pub struct StagedPhash {
    job_id: u32,
    prev_hash: binary_sv2::U256Owned,
    min_ntime: u32,
    nbits: u32,
}

impl StagedPhash {
    /// Converts a Staged PrevHash into a SetNewPrevHash message
    pub fn into_set_p_hash(&self, channel_id: u32, new_job_id: Option<u32>) -> SetNewPrevHashOwned {
        SetNewPrevHashOwned {
            channel_id,
            job_id: new_job_id.unwrap_or(self.job_id),
            prev_hash: self.prev_hash.clone(),
            min_ntime: self.min_ntime,
            nbits: self.nbits,
        }
    }
}

impl Share {
    /// Get share sequence number
    pub fn get_sequence_number(&self) -> u32 {
        match self {
            Share::Extended(s) => s.sequence_number,
            Share::Standard(s) => s.0.sequence_number,
        }
    }

    /// Get share channel id
    pub fn get_channel_id(&self) -> u32 {
        match self {
            Share::Extended(s) => s.channel_id,
            Share::Standard(s) => s.0.channel_id,
        }
    }

    /// Get share timestamp
    pub fn get_n_time(&self) -> u32 {
        match self {
            Share::Extended(s) => s.ntime,
            Share::Standard(s) => s.0.ntime,
        }
    }

    /// Get share nonce
    pub fn get_nonce(&self) -> u32 {
        match self {
            Share::Extended(s) => s.nonce,
            Share::Standard(s) => s.0.nonce,
        }
    }

    /// Get share job id
    pub fn get_job_id(&self) -> u32 {
        match self {
            Share::Extended(s) => s.job_id,
            Share::Standard(s) => s.0.job_id,
        }
    }

    /// Get share version
    pub fn get_version(&self) -> u32 {
        match self {
            Share::Extended(s) => s.version,
            Share::Standard(s) => s.0.version,
        }
    }
}

#[derive(Debug)]
/// Basic logic shared between all the channel factories
struct ChannelFactory {
    ids: Arc<Mutex<GroupId>>,
    standard_channels_for_non_hom_downstreams:
        HashMap<u64, StandardChannel, BuildNoHashHasher<u64>>,
    standard_channels_for_hom_downstreams: HashMap<u32, StandardChannel, BuildNoHashHasher<u32>>,
    extended_channels: HashMap<u32, OpenExtendedMiningChannelSuccessOwned, BuildNoHashHasher<u32>>,
    extranonces: ExtendedExtranonce,
    share_per_min: f32,
    // (NewExtendedMiningJob,group ids that already received the future job)
    future_jobs: Vec<(NewExtendedMiningJobOwned, Vec<u32>)>,
    // (SetNewPrevHash,group ids that already received the set prev_hash)
    last_prev_hash: Option<(StagedPhash, Vec<u32>)>,
    last_prev_hash_: Option<hash_types::BlockHash>,
    // (NewExtendedMiningJob,group ids that already received the job)
    last_valid_jobs: [Option<(NewExtendedMiningJobOwned, Vec<u32>)>; 3],
    // Index of the last valid job for channel_id ++ job_id
    id_to_job: HashMap<u32, u8, BuildNoHashHasher<u64>>,
    extended_job_variants: HashMap<(u32, u32), Vec<u8>>,
    extended_job_coinbase_outputs: HashMap<(u32, u32), TxOut>,
    // Used to understand which is the last added element in last_valid_jobs
    added_elements: usize,
    kind: ExtendedChannelKind,
    job_ids: Id,
    channel_to_group_id: HashMap<u32, u32, BuildNoHashHasher<u32>>,
    future_templates: HashMap<u32, NewTemplateOwned, BuildNoHashHasher<u32>>,
}

impl ChannelFactory {
    fn close_channel(&mut self, channel_id: u32) {
        if self.channel_to_group_id.get(&channel_id) == Some(&channel_id) {
            return;
        }
        self.standard_channels_for_hom_downstreams
            .remove(&channel_id);
        self.extended_channels.remove(&channel_id);
        self.extended_job_variants
            .retain(|(variant_channel_id, _), _| *variant_channel_id != channel_id);
        self.extended_job_coinbase_outputs
            .retain(|(variant_channel_id, _), _| *variant_channel_id != channel_id);
        if let Some(group_id) = self.channel_to_group_id.remove(&channel_id) {
            self.standard_channels_for_non_hom_downstreams
                .remove(&GroupId::into_complete_id(group_id, channel_id));
            self.remove_empty_group(group_id);
        }
    }

    fn remove_empty_group(&mut self, group_id: u32) {
        if group_id == 0
            || self
                .channel_to_group_id
                .iter()
                .any(|(channel_id, group)| *channel_id != group_id && *group == group_id)
        {
            return;
        }
        self.channel_to_group_id.remove(&group_id);
        for (_, sent_groups) in &mut self.future_jobs {
            sent_groups.retain(|id| *id != group_id);
        }
        for (_, sent_groups) in self.last_valid_jobs.iter_mut().flatten() {
            sent_groups.retain(|id| *id != group_id);
        }
        if let Some((_, sent_groups)) = &mut self.last_prev_hash {
            sent_groups.retain(|id| *id != group_id);
        }
    }

    fn ensure_channel_id_available(&self, channel_id: u32) -> Result<(), Error> {
        if channel_id == 0 || self.channel_to_group_id.contains_key(&channel_id) {
            return Err(Error::ChannelIdAlreadyInUse(channel_id));
        }
        Ok(())
    }

    fn ensure_group_id_available(&self, group_id: u32) -> Result<(), Error> {
        if self
            .channel_to_group_id
            .get(&group_id)
            .is_some_and(|id| *id != group_id)
        {
            return Err(Error::ChannelIdAlreadyInUse(group_id));
        }
        Ok(())
    }

    fn reserve_group(&mut self, group_id: u32) -> Result<(), Error> {
        self.ensure_group_id_available(group_id)?;
        self.ids
            .safe_lock(|ids| ids.reserve_channel_id(group_id))
            .map_err(|e| Error::PoisonLock(e.to_string()))?;
        self.channel_to_group_id.insert(group_id, group_id);
        Ok(())
    }
}

impl ChannelFactory {
    fn add_valid_job(&mut self, job: NewExtendedMiningJobOwned, group_ids: Vec<u32>) {
        match self.last_valid_jobs {
            [None, None, None] => {
                self.id_to_job.insert(job.job_id, 0);
                self.last_valid_jobs[0] = Some((job, group_ids));
                self.added_elements = 1;
            }
            [Some(_), None, None] => {
                self.id_to_job.insert(job.job_id, 1);
                self.last_valid_jobs[1] = Some((job, group_ids));
                self.added_elements = 2;
            }
            [Some(_), Some(_), None] => {
                self.id_to_job.insert(job.job_id, 2);
                self.last_valid_jobs[2] = Some((job, group_ids));
                self.added_elements = 3;
            }
            [Some(_), Some(_), Some(_)] => {
                let to_remove = self.added_elements % 3;
                let evicted_job_id = self.last_valid_jobs[to_remove].as_ref().unwrap().0.job_id;
                self.extended_job_variants
                    .retain(|(_, job_id), _| *job_id != evicted_job_id);
                self.extended_job_coinbase_outputs
                    .retain(|(_, job_id), _| *job_id != evicted_job_id);
                self.id_to_job.retain(|_, v| *v != to_remove as u8);
                self.id_to_job.insert(job.job_id, to_remove as u8);
                self.last_valid_jobs[to_remove] = Some((job, group_ids));
                self.added_elements += 1;
            }
            _ => panic!("Internal error: invalid last_valid_jobs state"),
        }
    }
    fn get_valid_job(&self, job_id: u32) -> Option<&(NewExtendedMiningJobOwned, Vec<u32>)> {
        let index = self.id_to_job.get(&job_id)?;
        self.last_valid_jobs[*index as usize].as_ref()
    }
    fn get_last_valid_job(&self) -> Option<&(NewExtendedMiningJobOwned, Vec<u32>)> {
        let index = self.get_last_valid_job_index()?;
        self.last_valid_jobs[index as usize].as_ref()
    }
    fn get_mut_last_valid_job(&mut self) -> Option<&mut (NewExtendedMiningJobOwned, Vec<u32>)> {
        let index = self.get_last_valid_job_index()?;
        self.last_valid_jobs[index as usize].as_mut()
    }
    fn get_last_valid_job_index(&self) -> Option<u8> {
        match self.last_valid_jobs {
            [None, None, None] => None,
            [Some(_), None, None] => Some(0),
            [Some(_), Some(_), None] => Some(1),
            [Some(_), Some(_), Some(_)] => Some(2),
            _ => panic!("Internal error: invalid last_valid_jobs state"),
        }
    }
    fn clear_valid_jobs(&mut self) {
        self.last_valid_jobs = [None, None, None];
        self.id_to_job.clear();
        self.added_elements = 0;
    }
}

impl ChannelFactory {
    pub fn add_standard_channel(
        &mut self,
        request_id: u32,
        downstream_hash_rate: f32,
        is_header_only: bool,
        id: u32,
        additional_coinbase_script_data: Option<&[u8]>,
    ) -> Result<Vec<MiningOwned>, Error> {
        if is_header_only {
            self.ensure_channel_id_available(id)?;
            self.ids
                .safe_lock(|ids| ids.reserve_channel_id(id))
                .map_err(|e| Error::PoisonLock(e.to_string()))?;
        } else {
            self.reserve_group(id)?;
        }
        match is_header_only {
            true => self.new_standard_channel_for_hom_downstream(
                request_id,
                downstream_hash_rate,
                id,
                additional_coinbase_script_data,
            ),
            false => self.new_standard_channel_for_non_hom_downstream(
                request_id,
                downstream_hash_rate,
                id,
                additional_coinbase_script_data,
            ),
        }
    }

    /// Called when a `OpenExtendedMiningChannel` message is received.
    /// Here we save the downstream's target (based on hashrate) and the
    /// channel's extranonce details before returning the relevant SV2 mining messages
    /// to be sent downstream. For the mining messages, we will first return an
    /// `OpenExtendedMiningChannelSuccess` if the channel is successfully opened. Then we add
    /// the `NewExtendedMiningJob` and `SetNewPrevHash` messages if the relevant data is
    /// available. If the channel opening fails, we return `OpenExtendedMiningChannelError`.
    pub fn new_extended_channel(
        &mut self,
        request_id: u32,
        hash_rate: f32,
        min_extranonce_size: u16,
        additional_coinbase_script_data: Option<&[u8]>,
    ) -> Result<(Vec<MiningOwned>, Option<u32>), Error> {
        let extended_channels_group = 0;
        let max_extranonce_size = self.extranonces.get_range2_len() as u16;
        if min_extranonce_size <= max_extranonce_size {
            // SECURITY is very unlikely to finish the ids btw this unwrap could be used by an
            // attacker that want to disrupt the service maybe we should have a method
            // to reuse ids that are no longer connected?
            let channel_id = self
                .ids
                .safe_lock(|ids| ids.new_channel_id(extended_channels_group))
                .unwrap();
            let target = match crate::utils::hash_rate_to_target(
                hash_rate.into(),
                self.share_per_min.into(),
            ) {
                Ok(target) => target,
                Err(e) => {
                    error!(
                        "Impossible to get target: {:?}. Request id: {:?}",
                        e, request_id
                    );
                    return Err(e);
                }
            };
            let (extranonce, extranonce_with_stripped_data) = loop {
                let extranonce = self
                    .extranonces
                    .next_extended(max_extranonce_size as usize)
                    .ok_or(Error::ExtranonceSpaceEnded)?;
                let extranonce_with_stripped_data = extranonce
                    .into_prefix(self.extranonces.get_prefix_len(), &[])
                    .unwrap();
                if !self.has_extended_channel_extranonce_prefix(
                    extranonce_with_stripped_data.as_bytes(),
                ) {
                    break (extranonce, extranonce_with_stripped_data);
                }
            };
            let success_with_stirpped_extranonce_add_data = OpenExtendedMiningChannelSuccessOwned {
                group_channel_id: 0,
                request_id,
                channel_id,
                target: target.clone(),
                extranonce_size: max_extranonce_size,
                extranonce_prefix: extranonce_with_stripped_data,
            };
            self.channel_to_group_id.insert(channel_id, 0);
            self.extended_channels
                .insert(channel_id, success_with_stirpped_extranonce_add_data);
            let extranonce_prefix = extranonce
                .into_prefix(
                    self.extranonces.get_prefix_len(),
                    additional_coinbase_script_data.unwrap_or(&[]),
                )
                .unwrap();
            let success = OpenExtendedMiningChannelSuccessOwned {
                group_channel_id: 0,
                request_id,
                channel_id,
                target,
                extranonce_size: max_extranonce_size,
                extranonce_prefix,
            };
            let mut result = vec![MiningOwned::OpenExtendedMiningChannelSuccess(success)];
            self.prepare_extended_jobs_and_p_hash(&mut result, channel_id);
            Ok((result, Some(channel_id)))
        } else {
            Ok((
                vec![MiningOwned::OpenMiningChannelError(
                    OpenMiningChannelError::unsupported_extranonce_size(request_id).into_owned(),
                )],
                None,
            ))
        }
    }

    fn has_extended_channel_extranonce_prefix(&self, extranonce_prefix: &[u8]) -> bool {
        self.extended_channels
            .values()
            .any(|channel| channel.extranonce_prefix.as_bytes() == extranonce_prefix)
    }

    fn prepare_extended_jobs_and_p_hash(&self, result: &mut Vec<MiningOwned>, channel_id: u32) {
        if let Some((job, _)) = &self.get_last_valid_job() {
            let mut job = job.clone();
            job.channel_id = channel_id;
            job.set_future();
            let j_id = job.job_id;
            result.push(MiningOwned::NewExtendedMiningJob(job));
            if let Some((new_prev_hash, _)) = &self.last_prev_hash {
                let mut new_prev_hash = new_prev_hash.into_set_p_hash(channel_id, None);
                new_prev_hash.job_id = j_id;
                result.push(MiningOwned::SetNewPrevHash(new_prev_hash.clone()))
            };
        } else if let Some((new_prev_hash, _)) = &self.last_prev_hash {
            let new_prev_hash = new_prev_hash.into_set_p_hash(channel_id, None);
            result.push(MiningOwned::SetNewPrevHash(new_prev_hash.clone()))
        };
        for (job, _) in &self.future_jobs {
            let mut job = job.clone();
            job.channel_id = channel_id;
            result.push(MiningOwned::NewExtendedMiningJob(job))
        }
    }

    /// Retain native channel metadata when restoring a pool channel or a JD mock assignment.
    fn replicate_extended_channel(
        &mut self,
        channel: OpenExtendedMiningChannelSuccessOwned,
    ) -> Result<(), Error> {
        self.ensure_channel_id_available(channel.channel_id)?;
        if channel.channel_id == channel.group_channel_id {
            return Err(Error::ChannelIdAlreadyInUse(channel.channel_id));
        }
        self.reserve_group(channel.group_channel_id)?;
        self.ids
            .safe_lock(|ids| ids.reserve_channel_id(channel.channel_id))
            .map_err(|e| Error::PoisonLock(e.to_string()))?;
        self.channel_to_group_id
            .insert(channel.channel_id, channel.group_channel_id);
        self.extended_channels.insert(channel.channel_id, channel);
        Ok(())
    }

    /// Called when an `OpenStandardChannel` message is received for a header only mining channel.
    /// Here we save the downstream's target (based on hashrate) and and the
    /// channel's extranonce details before returning the relevant SV2 mining messages
    /// to be sent downstream.
    fn new_standard_channel_for_hom_downstream(
        &mut self,
        request_id: u32,
        downstream_hash_rate: f32,
        id: u32,
        additional_coinbase_script_data: Option<&[u8]>,
    ) -> Result<Vec<MiningOwned>, Error> {
        let hom_group_id = 0;
        let mut result = vec![];
        let channel_id = id;
        let target = match crate::utils::hash_rate_to_target(
            downstream_hash_rate.into(),
            self.share_per_min.into(),
        ) {
            Ok(target) => target,
            Err(e) => {
                error!(
                    "Impossible to get target: {:?}. Request id: {:?}",
                    e, request_id
                );
                return Err(e);
            }
        };
        let extranonce = self
            .extranonces
            .next_standard()
            .ok_or(Error::ExtranonceSpaceEnded)?;
        let standard_channel = StandardChannel {
            channel_id,
            group_id: hom_group_id,
            target: target.clone().into(),
            extranonce: extranonce.clone(),
        };
        self.standard_channels_for_hom_downstreams
            .insert(channel_id, standard_channel);

        let extranonce: Vec<u8> = match additional_coinbase_script_data {
            Some(data) => {
                let mut data = data.to_vec();
                data.extend_from_slice(extranonce.as_ref());
                data
            }
            None => extranonce.into(),
        };

        // First message to be sent is OpenStandardMiningChannelSuccess
        result.push(MiningOwned::OpenStandardMiningChannelSuccess(
            OpenStandardMiningChannelSuccessOwned {
                request_id,
                channel_id,
                target,
                extranonce_prefix: extranonce.try_into().expect("Internal error: On initialization we make sure that extranonce + coinbase script additional data are not > then 32 bytes"),
                group_channel_id: hom_group_id,
            },
        ));
        self.prepare_standard_jobs_and_p_hash(
            &mut result,
            channel_id,
            additional_coinbase_script_data,
        )?;
        self.channel_to_group_id.insert(channel_id, hom_group_id);
        Ok(result)
    }

    /// This function is called when downstream have a group channel
    /// should not all standard channel's be non HOM??
    fn new_standard_channel_for_non_hom_downstream(
        &mut self,
        request_id: u32,
        downstream_hash_rate: f32,
        group_id: u32,
        additional_coinbase_script_data: Option<&[u8]>,
    ) -> Result<Vec<MiningOwned>, Error> {
        let mut result = vec![];
        let channel_id = self
            .ids
            .safe_lock(|ids| ids.new_channel_id(group_id))
            .unwrap();
        let complete_id = GroupId::into_complete_id(group_id, channel_id);
        let target = match crate::utils::hash_rate_to_target(
            downstream_hash_rate.into(),
            self.share_per_min.into(),
        ) {
            Ok(target_) => target_,
            Err(e) => {
                info!(
                    "Impossible to get target: {:?}. Request id: {:?}",
                    e, request_id
                );
                return Err(e);
            }
        };
        let extranonce = self
            .extranonces
            .next_standard()
            .ok_or(Error::ExtranonceSpaceEnded)?;
        let standard_channel = StandardChannel {
            channel_id,
            group_id,
            target: target.clone().into(),
            extranonce: extranonce.clone(),
        };
        self.standard_channels_for_non_hom_downstreams
            .insert(complete_id, standard_channel);

        let extranonce: Vec<u8> = match additional_coinbase_script_data {
            Some(data) => {
                let mut data = data.to_vec();
                data.extend_from_slice(extranonce.as_ref());
                data
            }
            None => extranonce.into(),
        };
        // First message to be sent is OpenStandardMiningChannelSuccess
        result.push(MiningOwned::OpenStandardMiningChannelSuccess(
            OpenStandardMiningChannelSuccessOwned {
                request_id,
                channel_id,
                target,
                extranonce_prefix: extranonce.try_into().expect(""),
                group_channel_id: group_id,
            },
        ));
        self.prepare_jobs_and_p_hash(&mut result, complete_id);
        self.channel_to_group_id.insert(channel_id, group_id);
        Ok(result)
    }

    // When a hom downstream opens a channel, we use this function to prepare all the standard jobs
    // (future and not) that we need to be sent downstream
    fn prepare_standard_jobs_and_p_hash(
        &mut self,
        result: &mut Vec<MiningOwned>,
        channel_id: u32,
        additional_coinbase_script_data: Option<&[u8]>,
    ) -> Result<(), Error> {
        // Safe cause the function is private and we always add the channel before calling this
        // funtion
        let standard_channel = self
            .standard_channels_for_hom_downstreams
            .get(&channel_id)
            .unwrap();
        // OPTIMIZATION this could be memoized somewhere cause is very likely that we will receive a
        // lot od OpenStandardMiningChannel requests consecutively
        let job_id = self.job_ids.next();
        let future_jobs: Option<Vec<NewMiningJobOwned>> = self
            .future_jobs
            .iter()
            .map(|j| {
                extended_to_standard_job(
                    &j.0,
                    standard_channel.extranonce.as_ref(),
                    standard_channel.channel_id,
                    Some(job_id),
                    additional_coinbase_script_data,
                )
            })
            .collect();

        let id = self.job_ids.next();
        // OPTIMIZATION the extranonce is cloned so many time but maybe is avoidable?
        let last_valid_job = match self.get_last_valid_job() {
            Some((j, _)) => Some(
                extended_to_standard_job(
                    j,
                    standard_channel.extranonce.as_ref(),
                    standard_channel.channel_id,
                    Some(id),
                    additional_coinbase_script_data,
                )
                .ok_or(Error::ImpossibleToCalculateMerkleRoot)?,
            ),
            None => None,
        };

        // This is the same thing of just check if there is a prev hash add it to result. If there
        // is last_job add it to result and add each future job to result.
        // But using the pattern match is more clear how each option is handled
        match (
            &self.last_prev_hash,
            last_valid_job,
            self.future_jobs.is_empty(),
        ) {
            // If we do not have anything just do nothing
            (None, None, true) => Ok(()),
            // If we have only future jobs we need to send them all after the
            // SetupConnectionSuccess message
            (None, None, false) => {
                // Safe unwrap cause we check that self.future_jobs is not empty
                let mut future_jobs = future_jobs.unwrap();
                while let Some(job) = future_jobs.pop() {
                    result.push(MiningOwned::NewMiningJob(job));
                }
                Ok(())
            }
            // If we have just a prev hash we need to send it after the SetupConnectionSuccess
            // message
            (Some((prev_h, _)), None, true) => {
                let prev_h = prev_h.into_set_p_hash(channel_id, None);
                result.push(MiningOwned::SetNewPrevHash(prev_h.clone()));
                Ok(())
            }
            // If we have a prev hash and a last valid job we need to send new mining job before the
            // prev hash
            (Some((prev_h, _)), Some(mut job), true) => {
                let prev_h = prev_h.into_set_p_hash(channel_id, Some(job.job_id));

                // set future_job to true
                job.set_future();

                result.push(MiningOwned::NewMiningJob(job));
                result.push(MiningOwned::SetNewPrevHash(prev_h.clone()));
                Ok(())
            }
            // If we have everything we need, send the future jobs and the the prev hash
            (Some((prev_h, _)), Some(mut job), false) => {
                let prev_h = prev_h.into_set_p_hash(channel_id, Some(job.job_id));

                job.set_future();

                result.push(MiningOwned::NewMiningJob(job));
                result.push(MiningOwned::SetNewPrevHash(prev_h.clone()));

                // Safe unwrap cause we check that self.future_jobs is not empty
                let mut future_jobs = future_jobs.unwrap();

                while let Some(job) = future_jobs.pop() {
                    result.push(MiningOwned::NewMiningJob(job));
                }
                Ok(())
            }
            // This can not happen because we can not have a valid job without a prev hash
            (None, Some(_), true) => unreachable!(),
            // This can not happen because we can not have a valid job without a prev hash
            (None, Some(_), false) => unreachable!(),
            // This can not happen because as soon as a prev hash is received we flush the future
            // jobs
            (Some(_), None, false) => unreachable!(),
        }
    }

    // When a new non HOM downstream opens a channel, we use this function to prepare all the
    // extended jobs (future and non) and the prev hash that we need to send downstream
    fn prepare_jobs_and_p_hash(&mut self, result: &mut Vec<MiningOwned>, complete_id: u64) {
        // If group is 0 it means that we are preparing jobs and p hash for a non HOM downstream
        // that want to open a new extended channel in that case we want to use the channel id
        // TODO verify that this is true also for the case where the channel factory is in a proxy
        // and not in a pool.
        let group_id = match GroupId::into_group_id(complete_id) {
            0 => GroupId::into_channel_id(complete_id),
            a => a,
        };
        // This is the same thing of just check if there is a prev hash add it to result if there
        // is last_job add it to result and add each future job to result.
        // But using the pattern match is more clear how each option is handled
        let last_prev_hash = self.last_prev_hash.clone();
        let is_empty = self.future_jobs.is_empty();
        match (last_prev_hash, self.get_mut_last_valid_job(), is_empty) {
            // If we do not have anything just do nothing
            (None, None, true) => (),
            // If we have only future jobs we need to send them all after the
            // SetupConnectionSuccess message
            (None, None, false) => {
                for (job, group_id_job_sent) in &mut self.future_jobs {
                    if !group_id_job_sent.contains(&group_id) {
                        let mut job = job.clone();
                        job.channel_id = group_id;
                        group_id_job_sent.push(group_id);
                        result.push(MiningOwned::NewExtendedMiningJob(job));
                    }
                }
            }
            // If we have just a prev hash we need to send it after the SetupConnectionSuccess
            // message
            (Some((prev_h, mut group_id_p_hash_sent)), None, true) => {
                if !group_id_p_hash_sent.contains(&group_id) {
                    let prev_h = prev_h.into_set_p_hash(group_id, None);
                    group_id_p_hash_sent.push(group_id);
                    result.push(MiningOwned::SetNewPrevHash(prev_h.clone()));
                }
                self.last_prev_hash = Some((prev_h, group_id_p_hash_sent));
            }
            // If we have a prev hash and a last valid job we need to send before the prev hash and
            // the the valid job
            (Some((prev_h, mut group_id_p_hash_sent)), Some((job, group_id_job_sent)), true) => {
                if !group_id_p_hash_sent.contains(&group_id) {
                    let prev_h = prev_h.into_set_p_hash(group_id, Some(job.job_id));
                    group_id_p_hash_sent.push(group_id);
                    result.push(MiningOwned::SetNewPrevHash(prev_h));
                }
                if !group_id_job_sent.contains(&group_id) {
                    let mut job = job.clone();
                    job.channel_id = group_id;
                    group_id_job_sent.push(group_id);
                    result.push(MiningOwned::NewExtendedMiningJob(job));
                }
                self.last_prev_hash = Some((prev_h, group_id_p_hash_sent));
            }
            // If we have everything we need, send before the prev hash and then all the jobs
            (Some((prev_h, mut group_id_p_hash_sent)), Some((job, group_id_job_sent)), false) => {
                if !group_id_p_hash_sent.contains(&group_id) {
                    let prev_h = prev_h.into_set_p_hash(group_id, Some(job.job_id));
                    group_id_p_hash_sent.push(group_id);
                    result.push(MiningOwned::SetNewPrevHash(prev_h));
                }

                if !group_id_job_sent.contains(&group_id) {
                    let mut job = job.clone();
                    job.channel_id = group_id;
                    group_id_job_sent.push(group_id);
                    result.push(MiningOwned::NewExtendedMiningJob(job));
                }

                for (job, group_id_future_j_sent) in &mut self.future_jobs {
                    if !group_id_future_j_sent.contains(&group_id) {
                        let mut job = job.clone();
                        job.channel_id = group_id;
                        group_id_future_j_sent.push(group_id);
                        result.push(MiningOwned::NewExtendedMiningJob(job));
                    }
                }
                self.last_prev_hash = Some((prev_h, group_id_p_hash_sent));
            }
            // This can not happen because we can not have a valid job without a prev hash
            (None, Some(_), true) => unreachable!(),
            // This can not happen because we can not have a valid job without a prev hash
            (None, Some(_), false) => unreachable!(),
            // This can not happen because as soon as a prev hash is received we flush the future
            // jobs
            (Some(_), None, false) => unreachable!(),
        }
    }

    /// Called when a new prev hash is received. If the respective job is available in the future
    /// job queue, we move the future job into the valid job slot and store the prev hash as the
    /// current prev hash to be referenced.
    fn on_new_prev_hash(&mut self, m: StagedPhash) -> Result<(), Error> {
        self.clear_valid_jobs();
        while let Some(mut job) = self.future_jobs.pop() {
            if job.0.job_id == m.job_id {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as u32;
                job.0.set_no_future(now);
                self.add_valid_job(job.0, job.1);
                break;
            }
        }
        self.future_jobs = vec![];
        let id_to_job = &self.id_to_job;
        self.extended_job_variants
            .retain(|(_, job_id), _| id_to_job.contains_key(job_id));
        self.extended_job_coinbase_outputs
            .retain(|(_, job_id), _| id_to_job.contains_key(job_id));
        self.last_prev_hash_ = Some(crate::utils::u256_to_block_hash(m.prev_hash.clone()));
        let mut ids = vec![];
        for complete_id in self.standard_channels_for_non_hom_downstreams.keys() {
            let group_id = GroupId::into_group_id(*complete_id);
            if !ids.contains(&group_id) {
                ids.push(group_id)
            }
        }
        self.last_prev_hash = Some((m, ids));
        Ok(())
    }

    /// Called when a `NewExtendedMiningJob` arrives. If the job is future, we add it to the future
    /// queue. If the job is not future, we pair it with a the most recent prev hash
    fn on_new_extended_mining_job(
        &mut self,
        m: NewExtendedMiningJobOwned,
        additional_coinbase_script_data: Option<&[u8]>,
    ) -> Result<HashMap<u32, MiningOwned, BuildNoHashHasher<u32>>, Error> {
        match (m.is_future(), &self.last_prev_hash) {
            (true, _) => {
                let mut result = HashMap::with_hasher(BuildNoHashHasher::default());
                self.prepare_jobs_for_downstream_on_new_extended(
                    &mut result,
                    &m,
                    additional_coinbase_script_data,
                )?;
                let mut ids = vec![];
                for complete_id in self.standard_channels_for_non_hom_downstreams.keys() {
                    let group_id = GroupId::into_group_id(*complete_id);
                    if !ids.contains(&group_id) {
                        ids.push(group_id)
                    }
                }
                self.future_jobs.push((m, ids));
                Ok(result)
            }
            (false, Some(_)) => {
                let mut result = HashMap::with_hasher(BuildNoHashHasher::default());
                self.prepare_jobs_for_downstream_on_new_extended(
                    &mut result,
                    &m,
                    additional_coinbase_script_data,
                )?;
                // If job is not future it must always be paired with the last received prev hash
                let mut ids = vec![];
                for complete_id in self.standard_channels_for_non_hom_downstreams.keys() {
                    let group_id = GroupId::into_group_id(*complete_id);
                    if !ids.contains(&group_id) {
                        ids.push(group_id)
                    }
                }
                self.add_valid_job(m, ids);
                if let Some((_p_hash, _)) = &self.last_prev_hash {
                    Ok(result)
                } else {
                    Err(Error::JobIsNotFutureButPrevHashNotPresent)
                }
            }
            // This should not happen when a non future job is received we always need to have a
            // prev hash
            (false, None) => Err(Error::JobIsNotFutureButPrevHashNotPresent),
        }
    }

    // When a new extended job is received we use this function to prepare the jobs to be sent
    // downstream (standard for hom and this job for non hom)
    fn prepare_jobs_for_downstream_on_new_extended(
        &mut self,
        result: &mut HashMap<u32, MiningOwned, BuildNoHashHasher<u32>>,
        m: &NewExtendedMiningJobOwned,
        additional_coinbase_script_data: Option<&[u8]>,
    ) -> Result<(), Error> {
        self.prepare_standard_jobs_for_downstream_on_new_extended(
            result,
            m,
            additional_coinbase_script_data,
        )?;
        self.prepare_extended_jobs_for_downstream_on_new_extended(result, m);
        Ok(())
    }

    fn prepare_standard_jobs_for_downstream_on_new_extended(
        &mut self,
        result: &mut HashMap<u32, MiningOwned, BuildNoHashHasher<u32>>,
        m: &NewExtendedMiningJobOwned,
        additional_coinbase_script_data: Option<&[u8]>,
    ) -> Result<(), Error> {
        for (id, channel) in &self.standard_channels_for_hom_downstreams {
            let job_id = self.job_ids.next();
            let mut standard_job = extended_to_standard_job(
                m,
                channel.extranonce.as_ref(),
                *id,
                Some(job_id),
                // We dont' care about what we have in the additional data since downstream do not
                // have to handle extranonces in that case, whatever is ok.
                additional_coinbase_script_data,
            )
            .unwrap();
            standard_job.channel_id = *id;
            let standard_job = MiningOwned::NewMiningJob(standard_job);
            result.insert(*id, standard_job);
        }
        for id in self.standard_channels_for_non_hom_downstreams.keys() {
            let group_id = GroupId::into_group_id(*id);
            let mut extended = m.clone();
            extended.channel_id = group_id;
            let extended_job = MiningOwned::NewExtendedMiningJob(extended);
            result.insert(group_id, extended_job);
        }
        Ok(())
    }

    fn prepare_extended_jobs_for_downstream_on_new_extended(
        &self,
        result: &mut HashMap<u32, MiningOwned, BuildNoHashHasher<u32>>,
        m: &NewExtendedMiningJobOwned,
    ) {
        for id in self.extended_channels.keys() {
            let mut extended = m.clone();
            extended.channel_id = *id;
            let extended_job = MiningOwned::NewExtendedMiningJob(extended);
            result.insert(*id, extended_job);
        }
    }

    // If there is job creator, bitcoin_target is retrieved from there. If not, it is set to 0.
    // If there is a job creator we pass the correct template id. If not, we pass `None`
    // allow comparison chain because clippy wants to make job management assertion into a match
    // clause
    #[allow(clippy::comparison_chain)]
    #[allow(clippy::too_many_arguments)]
    fn check_target<TxHash: std::convert::AsRef<[u8]>>(
        &mut self,
        mut m: Share,
        bitcoin_target: Target,
        template_id: Option<u64>,
        up_id: u32,
        merkle_path: Vec<TxHash>,
        coinbase_tx_prefix: &[u8],
        coinbase_tx_suffix: &[u8],
        prev_blockhash: hash_types::BlockHash,
        bits: u32,
        additional_coinbase_script_data: Option<&[u8]>,
        job_version: u32,
        version_rolling_allowed: bool,
    ) -> Result<OnNewShare, Error> {
        let rolling = matches!(m, Share::Standard(_)) || version_rolling_allowed;
        if let Err(code) = crate::version::validate_version(job_version, m.get_version(), rolling) {
            return Ok(OnNewShare::SendErrorDownstream(SubmitSharesErrorOwned {
                channel_id: m.get_channel_id(),
                sequence_number: m.get_sequence_number(),
                error_code: code.to_string().try_into()?,
            }));
        }
        debug!("Checking target for share {:?}", m);
        let upstream_target = match &self.kind {
            ExtendedChannelKind::Pool => Target::new(0, 0),
            ExtendedChannelKind::Proxy {
                upstream_target, ..
            }
            | ExtendedChannelKind::ProxyJd {
                upstream_target, ..
            } => upstream_target.clone(),
        };

        let (downstream_target, extranonce) = self.get_channel_specific_mining_info(&m)?;
        let extranonce_1_len = self.extranonces.get_range0_len();
        let extranonce_2 = extranonce[extranonce_1_len..].to_vec();
        match &mut m {
            Share::Extended(extended_share) => {
                extended_share.extranonce = extranonce_2.try_into()?;
            }
            Share::Standard(_) => (),
        };
        trace!(
            "On checking target coinbase prefix is: {:?}",
            coinbase_tx_prefix.to_hex_string(hex::Case::Lower)
        );
        trace!(
            "On checking target coinbase suffix is: {:?}",
            coinbase_tx_suffix.to_hex_string(hex::Case::Lower)
        );
        // Safe unwrap a sha256 can always be converted into [u8;32]
        let merkle_root: [u8; 32] = crate::utils::merkle_root_from_path(
            coinbase_tx_prefix,
            coinbase_tx_suffix,
            &extranonce[..],
            &merkle_path[..],
            additional_coinbase_script_data.unwrap_or(&[]),
        )
        .ok_or(Error::InvalidCoinbase)?
        .try_into()
        .unwrap();
        let version = match &m {
            Share::Extended(share) => share.version as i32,
            Share::Standard(share) => share.0.version as i32,
        };

        let header = Header {
            version: Version::from_consensus(version),
            prev_blockhash,
            merkle_root: (*Hash::from_bytes_ref(&merkle_root)).into(),
            time: m.get_n_time(),
            bits: CompactTarget::from_consensus(bits),
            nonce: m.get_nonce(),
        };

        trace!("On checking target header is: {:?}", header);
        let hash_ = header.block_hash();
        let hash: [u8; 32] = *hash_.to_raw_hash().as_ref();

        if tracing::level_enabled!(tracing::Level::DEBUG)
            || tracing::level_enabled!(tracing::Level::TRACE)
        {
            let bitcoin_target_log: binary_sv2::U256Owned = bitcoin_target.clone().into();
            let mut bitcoin_target_log = bitcoin_target_log.into_bytes();
            bitcoin_target_log.reverse();
            debug!("Bitcoin target : {:?}", bitcoin_target_log.as_hex());
            let upstream_target: binary_sv2::U256Owned = upstream_target.clone().into();
            let mut upstream_target = upstream_target.into_bytes();
            upstream_target.reverse();
            debug!("Upstream target: {:?}", upstream_target.as_hex());
            let mut hash = hash;
            hash.reverse();
            debug!("Hash           : {:?}", hash.to_vec().as_hex());
        }
        let hash: Target = hash.into();

        if hash <= bitcoin_target {
            let mut print_hash: [u8; 32] = *hash_.to_raw_hash().as_ref();
            print_hash.reverse();

            info!(
                "Share hash meet bitcoin target: {:?}",
                print_hash.to_vec().as_hex()
            );

            let coinbase = [
                coinbase_tx_prefix,
                additional_coinbase_script_data.unwrap_or(&[]),
                &extranonce[..],
                coinbase_tx_suffix,
            ]
            .concat()
            .to_vec();
            match self.kind {
                ExtendedChannelKind::Proxy { .. } | ExtendedChannelKind::ProxyJd { .. } => {
                    let upstream_extranonce_space = self.extranonces.get_range0_len();
                    let extranonce_ = extranonce[upstream_extranonce_space..].to_vec();
                    let mut res = OnNewShare::ShareMeetBitcoinTarget((
                        m,
                        template_id,
                        coinbase,
                        extranonce.to_vec(),
                    ));
                    res.into_extended(extranonce_, up_id);
                    Ok(res)
                }
                ExtendedChannelKind::Pool => Ok(OnNewShare::ShareMeetBitcoinTarget((
                    m,
                    template_id,
                    coinbase,
                    extranonce.to_vec(),
                ))),
            }
        } else if hash <= upstream_target {
            match self.kind {
                ExtendedChannelKind::Proxy { .. } | ExtendedChannelKind::ProxyJd { .. } => {
                    let upstream_extranonce_space = self.extranonces.get_range0_len();
                    let extranonce = extranonce[upstream_extranonce_space..].to_vec();
                    let mut res = OnNewShare::SendSubmitShareUpstream((m, template_id));
                    res.into_extended(extranonce, up_id);
                    Ok(res)
                }
                ExtendedChannelKind::Pool => {
                    Ok(OnNewShare::SendSubmitShareUpstream((m, template_id)))
                }
            }
        } else if hash <= downstream_target {
            Ok(OnNewShare::ShareMeetDownstreamTarget)
        } else {
            error!("Share does not meet any target: {:?}", m);
            let error = SubmitSharesErrorOwned {
                channel_id: m.get_channel_id(),
                sequence_number: m.get_sequence_number(),
                // Infallible unwrap we already know the len of the error code (is a
                // static string)
                error_code: mining_sv2::ERROR_CODE_SUBMIT_SHARES_DIFFICULTY_TOO_LOW
                    .to_string()
                    .try_into()
                    .unwrap(),
            };
            Ok(OnNewShare::SendErrorDownstream(error))
        }
    }

    /// Returns the downstream target and extranonce for the channel
    fn get_channel_specific_mining_info(
        &self,
        m: &Share,
    ) -> Result<(crate::mining::Target, Vec<u8>), Error> {
        match m {
            Share::Extended(share) => {
                let channel = self
                    .extended_channels
                    .get(&m.get_channel_id())
                    .ok_or(Error::ShareDoNotMatchAnyChannel)?;
                if share.extranonce.as_bytes().len() != usize::from(channel.extranonce_size) {
                    return Err(Error::InvalidCoinbase);
                }
                let dowstream_target = channel.target.clone().into();
                let extranonce = [
                    channel.extranonce_prefix.as_bytes(),
                    share.extranonce.as_bytes(),
                ]
                .concat();
                Ok((dowstream_target, extranonce))
            }
            Share::Standard((share, group_id)) => match &self.kind {
                ExtendedChannelKind::Pool => {
                    let complete_id = GroupId::into_complete_id(*group_id, share.channel_id);
                    let mut channel = self
                        .standard_channels_for_non_hom_downstreams
                        .get(&complete_id);
                    if channel.is_none() {
                        channel = self
                            .standard_channels_for_hom_downstreams
                            .get(&share.channel_id);
                    };
                    Ok((
                        channel
                            .ok_or(Error::ShareDoNotMatchAnyChannel)?
                            .target
                            .clone(),
                        channel
                            .ok_or(Error::ShareDoNotMatchAnyChannel)?
                            .extranonce
                            .clone()
                            .to_vec(),
                    ))
                }
                ExtendedChannelKind::Proxy { .. } | ExtendedChannelKind::ProxyJd { .. } => {
                    let complete_id = GroupId::into_complete_id(*group_id, share.channel_id);
                    let mut channel = self
                        .standard_channels_for_non_hom_downstreams
                        .get(&complete_id);
                    if channel.is_none() {
                        channel = self
                            .standard_channels_for_hom_downstreams
                            .get(&share.channel_id);
                    };
                    Ok((
                        channel
                            .ok_or(Error::ShareDoNotMatchAnyChannel)?
                            .target
                            .clone(),
                        channel
                            .ok_or(Error::ShareDoNotMatchAnyChannel)?
                            .extranonce
                            .clone()
                            .to_vec(),
                    ))
                }
            },
        }
    }
    /// Updates the downstream target for the given channel_id
    fn update_target_for_channel(&mut self, channel_id: u32, new_target: Target) -> Option<bool> {
        let channel = self.extended_channels.get_mut(&channel_id)?;
        channel.target = new_target.into();
        Some(true)
    }
}

type CustomJobs = HashMap<u32, SetCustomMiningJobOwned, BuildNoHashHasher<u32>>;

/// Maximum acknowledged custom jobs retained per channel, evicting the oldest first.
/// Native protocol limits also bound the size of each retained job.
pub const MAX_CUSTOM_JOBS_PER_CHANNEL: usize = 32;

/// Used by a pool to in order to manage all downstream channel. It adds job creation capabilities
/// to ChannelFactory.
#[derive(Debug)]
pub struct PoolChannelFactory {
    inner: ChannelFactory,
    job_creator: JobsCreators,
    pool_coinbase_outputs: Vec<TxOut>,
    last_channel_coinbase_sequence: Option<[u8; 24]>,
    channel_coinbase_outputs: HashMap<u32, ChannelCoinbaseOutput, BuildNoHashHasher<u32>>,
    user_identifier_to_channel: HashMap<Vec<u8>, u32>,
    // Per channel additional data that the pool may want to include in the coinbase input script
    // as first part of the extranonce. This can be used to put things like the pool signature
    // or commitments. It is per channel since the pool may want to include different
    // commitment data based on downstream hash rate.
    // channel_if -> (additional_coinbase_script_data, old_additional_coinbase_script_data)
    #[allow(clippy::type_complexity)]
    channel_to_additional_coinbase_script_data:
        HashMap<u32, (Vec<u8>, Option<Vec<u8>>), BuildNoHashHasher<u32>>,
    // Additional data that the pool may want to include in the coinbase input script as first part
    // of the extranonce. This can be used to put things like the pool signature.
    additional_coinbase_script_data: Vec<u8>,
    // channel_id ++ job_id
    job_ids_using_old_add_data: HashSet<u32, BuildNoHashHasher<u32>>,
    // extended_channel_id -> acknowledged job_id -> SetCustomMiningJob. Empty entries retain
    // custom-job mode until channel close, so evicted IDs cannot resolve to canonical pool jobs.
    negotiated_jobs: HashMap<u32, CustomJobs, BuildNoHashHasher<u32>>,
}

impl PoolChannelFactory {
    /// constructor
    pub fn new(
        ids: Arc<Mutex<GroupId>>,
        extranonces: ExtendedExtranonce,
        job_creator: JobsCreators,
        share_per_min: f32,
        kind: ExtendedChannelKind,
        pool_coinbase_outputs: Vec<TxOut>,
        additional_coinbase_script_data: Vec<u8>,
    ) -> Result<Self, Error> {
        if additional_coinbase_script_data.len() + extranonces.get_len() > 32 {
            error!("Additional coinbase script data is too big");
            return Err(Error::AdditionalCoinbaseScriptDataTooBig);
        }
        let inner = ChannelFactory {
            ids,
            standard_channels_for_non_hom_downstreams: HashMap::with_hasher(
                BuildNoHashHasher::default(),
            ),
            standard_channels_for_hom_downstreams: HashMap::with_hasher(
                BuildNoHashHasher::default(),
            ),
            extended_channels: HashMap::with_hasher(BuildNoHashHasher::default()),
            extranonces,
            share_per_min,
            future_jobs: Vec::new(),
            last_prev_hash: None,
            last_prev_hash_: None,
            last_valid_jobs: [None, None, None],
            id_to_job: HashMap::with_hasher(BuildNoHashHasher::default()),
            extended_job_variants: HashMap::new(),
            extended_job_coinbase_outputs: HashMap::new(),
            added_elements: 0,
            kind,
            job_ids: Id::new(),
            channel_to_group_id: HashMap::with_hasher(BuildNoHashHasher::default()),
            future_templates: HashMap::with_hasher(BuildNoHashHasher::default()),
        };

        Ok(Self {
            inner,
            job_creator,
            pool_coinbase_outputs,
            last_channel_coinbase_sequence: None,
            channel_coinbase_outputs: HashMap::with_hasher(BuildNoHashHasher::default()),
            user_identifier_to_channel: HashMap::new(),
            channel_to_additional_coinbase_script_data: HashMap::with_hasher(
                BuildNoHashHasher::default(),
            ),
            additional_coinbase_script_data,
            job_ids_using_old_add_data: HashSet::with_hasher(BuildNoHashHasher::default()),
            negotiated_jobs: HashMap::with_hasher(BuildNoHashHasher::default()),
        })
    }

    /// Enables CHSEQ outputs before any extended channels are opened. Disabled by default.
    /// Repeated calls preserve the existing factory prefix and counter.
    /// Every successful open/restoration gets a fresh counter under one random factory prefix.
    /// Standard jobs are unchanged. Custom jobs must include their assigned output exactly once.
    pub fn with_channel_coinbase_outputs(mut self) -> Self {
        assert!(
            self.last_channel_coinbase_sequence.is_some()
                || self.inner.extended_channels.is_empty()
        );
        self.last_channel_coinbase_sequence.get_or_insert_with(|| {
            let mut sequence = [0; 24];
            OsRng.fill_bytes(&mut sequence[..16]);
            sequence
        });
        self
    }

    /// Copies a channel's stable output for materializing fanout jobs outside the factory lock.
    pub fn channel_coinbase_output(&self, channel_id: u32) -> Option<ChannelCoinbaseOutput> {
        self.channel_coinbase_outputs.get(&channel_id).copied()
    }

    /// Registers a unique user identifier for an open extended channel. Repeating a pair is safe.
    pub fn register_channel_user(
        &mut self,
        channel_id: u32,
        user_identifier: &[u8],
    ) -> Result<(), Error> {
        if !self.inner.extended_channels.contains_key(&channel_id) {
            return Err(Error::NotFoundChannelId);
        }
        if let Some(existing_id) = self.user_identifier_to_channel.get(user_identifier) {
            if *existing_id != channel_id {
                return Err(Error::ChannelIdAlreadyInUse(*existing_id));
            }
        }
        self.user_identifier_to_channel
            .insert(user_identifier.to_vec(), channel_id);
        Ok(())
    }

    /// Returns a registered channel until it is closed.
    pub fn channel_id_for_user(&self, user_identifier: &[u8]) -> Option<u32> {
        self.user_identifier_to_channel
            .get(user_identifier)
            .copied()
    }

    fn next_channel_coinbase_output(&self) -> Result<Option<ChannelCoinbaseOutput>, Error> {
        self.last_channel_coinbase_sequence
            .map(|mut sequence| {
                for byte in sequence[16..].iter_mut().rev() {
                    let (next, carry) = byte.overflowing_add(1);
                    *byte = next;
                    if !carry {
                        return Ok(ChannelCoinbaseOutput(sequence));
                    }
                }
                Err(Error::ChannelCoinbaseSequenceExhausted)
            })
            .transpose()
    }

    fn materialize_extended_channel_open(
        output: Option<ChannelCoinbaseOutput>,
        messages: &mut [MiningOwned],
    ) -> Result<(), Error> {
        if let Some(output) = output {
            for message in messages {
                if let MiningOwned::NewExtendedMiningJob(job) = message {
                    output.apply(job)?;
                }
            }
        }
        Ok(())
    }

    fn finish_extended_channel_open(
        &mut self,
        channel_id: u32,
        output: Option<ChannelCoinbaseOutput>,
        messages: Vec<MiningOwned>,
    ) -> Vec<MiningOwned> {
        if let Some(output) = output {
            self.last_channel_coinbase_sequence = Some(output.0);
            self.channel_coinbase_outputs.insert(channel_id, output);
        }
        messages
    }

    /// Calls `ChannelFactory::add_standard_channel`
    pub fn add_standard_channel(
        &mut self,
        request_id: u32,
        downstream_hash_rate: f32,
        is_header_only: bool,
        id: u32,
    ) -> Result<Vec<MiningOwned>, Error> {
        if !is_header_only
            && self.inner.extended_channels.values().any(|channel| {
                channel.group_channel_id == id
                    && self
                        .channel_coinbase_outputs
                        .contains_key(&channel.channel_id)
            })
        {
            return Err(Error::IncompatibleChannelGroup(id));
        }
        let messages = self.inner.add_standard_channel(
            request_id,
            downstream_hash_rate,
            is_header_only,
            id,
            Some(&self.additional_coinbase_script_data),
        )?;
        if let Some(MiningOwned::OpenStandardMiningChannelSuccess(success)) = messages.first() {
            self.channel_to_additional_coinbase_script_data.insert(
                success.channel_id,
                (self.additional_coinbase_script_data.clone(), None),
            );
        }
        Ok(messages)
    }

    /// Calls `ChannelFactory::new_extended_channel`
    pub fn new_extended_channel(
        &mut self,
        request_id: u32,
        hash_rate: f32,
        min_extranonce_size: u16,
    ) -> Result<Vec<MiningOwned>, Error> {
        if self.last_channel_coinbase_sequence.is_some()
            && self
                .inner
                .standard_channels_for_non_hom_downstreams
                .values()
                .any(|channel| channel.group_id == 0)
        {
            return Err(Error::IncompatibleChannelGroup(0));
        }
        let output = self.next_channel_coinbase_output()?;
        match self.inner.new_extended_channel(
            request_id,
            hash_rate,
            min_extranonce_size,
            Some(&self.additional_coinbase_script_data),
        ) {
            // Channel is opened
            Ok((mut res, Some(channel_id))) => {
                if let Err(error) = Self::materialize_extended_channel_open(output, &mut res) {
                    self.close_channel(channel_id);
                    return Err(error);
                }
                self.channel_to_additional_coinbase_script_data.insert(
                    channel_id,
                    (self.additional_coinbase_script_data.clone(), None),
                );
                Ok(self.finish_extended_channel_open(channel_id, output, res))
            }
            // Channel is not opened and we can return an error downtream
            Ok((res, None)) => Ok(res),
            // Channel is not opened and we can not return an error downtream
            Err(e) => Err(e),
        }
    }

    /// Called when we want to replicate a channel already opened by another actor.
    /// is used only in the jd client from the template provider module to mock a pool.
    /// Anything else should open channel with the new_extended_channel function
    ///
    /// This does not register the replicated channel's extranonce in the `ExtendedExtranonce`
    /// allocator, so the factory can still allocate overlapping extranonce space later.
    /// SAFETY It is safe only when all of the following are true:
    /// - the factory is used only as a JD mock pool for share validation;
    /// - no standard or extended channels are opened from this factory after replication;
    /// - the channel id and extranonce come from a channel already opened by the trusted upstream;
    /// - no pool additional coinbase script data is configured.
    pub fn replicate_upstream_extended_channel_only_jd(
        &mut self,
        channel: OpenExtendedMiningChannelSuccessOwned,
    ) -> Result<(), Error> {
        if !self.additional_coinbase_script_data.is_empty() {
            return Err(Error::AdditionalCoinbaseScriptDataTooBig);
        }
        let full_len =
            channel.extranonce_prefix.as_bytes().len() + usize::from(channel.extranonce_size);
        if full_len > 32 {
            return Err(Error::InvalidExtranonceSize(
                32,
                full_len.try_into().unwrap_or(u16::MAX),
            ));
        }
        let channel_id = channel.channel_id;
        self.inner.replicate_extended_channel(channel)?;
        self.channel_to_additional_coinbase_script_data
            .insert(channel_id, (vec![], None));
        Ok(())
    }

    /// Called when a pool wants to restore an already-known extended channel prefix.
    ///
    /// The provided `extranonce_prefix` must not include the pool's additional coinbase script
    /// data. That data is stored separately and prepended only when constructing the coinbase.
    /// this function can be used also from downstream, but the additional_coinbase_script_data
    /// should be empty (that field non empty only if the pool has to restore a channel, with a
    /// pool signature already assigned in a previous session)
    pub fn replicate_pool_extended_channel(
        &mut self,
        request_id: u32,
        target: binary_sv2::U256Owned,
        extranonce_prefix: crate::mining::Extranonce,
        channel_id: u32,
        extranonce_size: u16,
        additional_coinbase_script_data: Vec<u8>,
    ) -> Result<Vec<MiningOwned>, Error> {
        self.restore_pool_channel(
            OpenExtendedMiningChannelSuccessOwned {
                request_id,
                channel_id,
                target,
                extranonce_prefix: extranonce_prefix.into(),
                extranonce_size,
                group_channel_id: 0,
            },
            additional_coinbase_script_data,
        )
    }

    fn restore_pool_channel(
        &mut self,
        channel: OpenExtendedMiningChannelSuccessOwned,
        additional_coinbase_script_data: Vec<u8>,
    ) -> Result<Vec<MiningOwned>, Error> {
        let output = self.next_channel_coinbase_output()?;
        if output.is_some()
            && self
                .inner
                .standard_channels_for_non_hom_downstreams
                .values()
                .any(|standard| standard.group_id == channel.group_channel_id)
        {
            return Err(Error::IncompatibleChannelGroup(channel.group_channel_id));
        }
        if self.additional_coinbase_script_data.len() != additional_coinbase_script_data.len() {
            return Err(Error::NewAdditionalCoinbaseDataLenDoNotMatch);
        }
        let expected_prefix_len = self.inner.extranonces.get_prefix_len();
        let actual_prefix_len = channel.extranonce_prefix.as_bytes().len();
        if actual_prefix_len != expected_prefix_len {
            return Err(Error::InvalidExtranonceSize(
                expected_prefix_len as u16,
                actual_prefix_len as u16,
            ));
        }
        let expected_extranonce_size = self.inner.extranonces.get_range2_len();
        if usize::from(channel.extranonce_size) != expected_extranonce_size {
            return Err(Error::InvalidExtranonceSize(
                expected_extranonce_size as u16,
                channel.extranonce_size,
            ));
        }
        self.inner.ensure_channel_id_available(channel.channel_id)?;
        for existing in self
            .inner
            .extended_channels
            .values()
            .filter(|existing| existing.extranonce_prefix == channel.extranonce_prefix)
        {
            match (output, self.channel_coinbase_output(existing.channel_id)) {
                (Some(restored), Some(existing)) if restored != existing => (),
                _ => return Err(Error::ExtranoncePrefixAlreadyInUse),
            }
        }
        let channel_id = channel.channel_id;
        let mut success = channel.clone();
        success.extranonce_prefix = [
            additional_coinbase_script_data.as_slice(),
            channel.extranonce_prefix.as_bytes(),
        ]
        .concat()
        .try_into()?;
        let mut result = vec![MiningOwned::OpenExtendedMiningChannelSuccess(success)];
        self.inner
            .prepare_extended_jobs_and_p_hash(&mut result, channel_id);
        Self::materialize_extended_channel_open(output, &mut result)?;
        self.inner.replicate_extended_channel(channel)?;
        self.channel_to_additional_coinbase_script_data
            .insert(channel_id, (additional_coinbase_script_data, None));
        Ok(self.finish_extended_channel_open(channel_id, output, result))
    }

    /// Restores native channel/group metadata and the advertised prefix including pool script data.
    /// With CHSEQ enabled, a restored channel gets a fresh marker and may reuse an occupied nonce
    /// prefix only when every colliding channel also has a distinct marker.
    pub fn restore_extended_channel(
        &mut self,
        mut channel: OpenExtendedMiningChannelSuccessOwned,
        additional_coinbase_script_data: Vec<u8>,
    ) -> Result<Vec<MiningOwned>, Error> {
        channel.extranonce_prefix = channel
            .extranonce_prefix
            .as_bytes()
            .strip_prefix(additional_coinbase_script_data.as_slice())
            .ok_or(Error::InvalidCoinbase)?
            .to_vec()
            .try_into()?;
        self.restore_pool_channel(channel, additional_coinbase_script_data)
    }

    /// Called only when a new prev hash is received by a Template Provider. It matches the
    /// message with a `job_id` and calls `ChannelFactory::on_new_prev_hash`
    /// it return the job_id
    pub fn on_new_prev_hash_from_tp(
        &mut self,
        m: &SetNewPrevHashFromTpOwned,
    ) -> Result<u32, Error> {
        let job_id = self.job_creator.on_new_prev_hash(m).unwrap_or(0);
        let new_prev_hash = StagedPhash {
            job_id,
            prev_hash: m.prev_hash.clone(),
            min_ntime: m.header_timestamp,
            nbits: m.n_bits,
        };
        self.inner.on_new_prev_hash(new_prev_hash)?;
        for jobs in self.negotiated_jobs.values_mut() {
            // The client may register work before its matching TP tip arrives. Keep that work
            // while discarding obsolete contexts, including their coinbase outputs and paths.
            jobs.retain(|_, job| job.prev_hash == m.prev_hash);
        }
        Ok(job_id)
    }

    /// Called only when a new template is received by a Template Provider
    pub fn on_new_template(
        &mut self,
        m: &mut NewTemplateOwned,
    ) -> Result<HashMap<u32, MiningOwned, BuildNoHashHasher<u32>>, Error> {
        if !m.future_template && self.inner.last_prev_hash.is_none() {
            return Err(Error::JobIsNotFutureButPrevHashNotPresent);
        }
        let marker = self.channel_coinbase_outputs.values().next().copied();
        let new_job = self.job_creator.on_new_template_with_validation(
            m,
            true,
            self.pool_coinbase_outputs.clone(),
            self.additional_coinbase_script_data.len() as u8,
            |job| match marker {
                Some(marker) => marker.apply(&mut job.clone()),
                None => Ok(()),
            },
        )?;
        let mut messages = self.inner.on_new_extended_mining_job(
            new_job,
            // Here we can use the data that we used to initialize this channel factory. Since this
            // value it will be used only to create standard jobs for HOM downstreams.
            Some(&self.additional_coinbase_script_data),
        )?;
        for message in messages.values_mut() {
            if let MiningOwned::NewExtendedMiningJob(job) = message {
                if let Some(output) = self.channel_coinbase_output(job.channel_id) {
                    output.apply(job)?;
                }
            }
        }
        Ok(messages)
    }

    /// Registers a template and returns the data needed to fan out extended jobs outside the
    /// caller's channel-factory lock. Apply [`Self::channel_coinbase_output`] to each cloned job
    /// before adding local outputs and registering any per-job output or suffix variant.
    #[allow(clippy::type_complexity)]
    pub fn on_new_template_for_extended_fanout(
        &mut self,
        m: &mut NewTemplateOwned,
    ) -> Result<
        (
            NewExtendedMiningJobOwned,
            Vec<u32>,
            HashMap<u32, MiningOwned, BuildNoHashHasher<u32>>,
        ),
        Error,
    > {
        if !m.future_template && self.inner.last_prev_hash.is_none() {
            return Err(Error::JobIsNotFutureButPrevHashNotPresent);
        }

        let marker = self.channel_coinbase_outputs.values().next().copied();
        let new_job = self.job_creator.on_new_template_with_validation(
            m,
            true,
            self.pool_coinbase_outputs.clone(),
            self.additional_coinbase_script_data.len() as u8,
            |job| match marker {
                Some(marker) => marker.apply(&mut job.clone()),
                None => Ok(()),
            },
        )?;
        let extended_channel_ids = self.inner.extended_channels.keys().copied().collect();
        let mut standard_messages = HashMap::with_hasher(BuildNoHashHasher::default());
        self.inner
            .prepare_standard_jobs_for_downstream_on_new_extended(
                &mut standard_messages,
                &new_job,
                Some(&self.additional_coinbase_script_data),
            )?;

        let mut ids = vec![];
        for complete_id in self.inner.standard_channels_for_non_hom_downstreams.keys() {
            let group_id = GroupId::into_group_id(*complete_id);
            if !ids.contains(&group_id) {
                ids.push(group_id)
            }
        }

        match (new_job.is_future(), &self.inner.last_prev_hash) {
            (true, _) => {
                self.inner.future_jobs.push((new_job.clone(), ids));
                Ok((new_job, extended_channel_ids, standard_messages))
            }
            (false, Some(_)) => {
                self.inner.add_valid_job(new_job.clone(), ids);
                Ok((new_job, extended_channel_ids, standard_messages))
            }
            (false, None) => unreachable!("non-future jobs require a prevhash"),
        }
    }

    /// Called when a `SubmitSharesStandard` message is received from the downstream. We check the
    /// shares against the channel's respective target and return `OnNewShare` to let us know if
    /// and where the shares should be relayed
    pub fn on_submit_shares_standard(
        &mut self,
        m: SubmitSharesStandard,
    ) -> Result<OnNewShare, Error> {
        let additional_coinbase_script_data = self
            .get_additional_coinbase_script_data(m.channel_id, m.job_id)
            .ok_or(Error::ShareDoNotMatchAnyJob)?;
        match self.inner.channel_to_group_id.get(&m.channel_id) {
            Some(g_id) => {
                let referenced_job = self
                    .inner
                    .get_valid_job(m.job_id)
                    .cloned()
                    .ok_or(Error::ShareDoNotMatchAnyJob)?
                    .0;
                let merkle_path = referenced_job.merkle_path.clone().into_inner();
                let template_id = self
                    .job_creator
                    .get_template_id_from_job(referenced_job.job_id)
                    .ok_or(Error::NoTemplateForId)?;
                let target = self.job_creator.last_target();
                let prev_blockhash = self
                    .inner
                    .last_prev_hash_
                    .ok_or(Error::ShareDoNotMatchAnyJob)?;
                let bits = self
                    .inner
                    .last_prev_hash
                    .as_ref()
                    .ok_or(Error::ShareDoNotMatchAnyJob)?
                    .0
                    .nbits;
                self.inner.check_target(
                    Share::Standard((m, *g_id)),
                    target,
                    Some(template_id),
                    0,
                    merkle_path,
                    referenced_job.coinbase_tx_prefix.as_ref(),
                    referenced_job.coinbase_tx_suffix.as_ref(),
                    prev_blockhash,
                    bits,
                    Some(&additional_coinbase_script_data),
                    referenced_job.version,
                    referenced_job.version_rolling_allowed,
                )
            }
            None => {
                let err = SubmitSharesErrorOwned {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: mining_sv2::ERROR_CODE_SUBMIT_SHARES_INVALID_CHANNEL_ID
                        .to_string()
                        .try_into()
                        .unwrap(),
                };
                Ok(OnNewShare::SendErrorDownstream(err))
            }
        }
    }

    /// Called when a `SubmitSharesExtended` message is received from the downstream. We check the
    /// shares against the channel's respective target and return `OnNewShare` to let us know if
    /// and where the shares should be relayed
    pub fn on_submit_shares_extended(
        &mut self,
        m: SubmitSharesExtendedOwned,
    ) -> Result<OnNewShare, Error> {
        let target = self.job_creator.last_target();
        let additional_coinbase_script_data = self
            .get_additional_coinbase_script_data(m.channel_id, m.job_id)
            .ok_or(Error::ShareDoNotMatchAnyJob)?;
        // Custom shares must reference an acknowledged job on this channel. Retain earlier
        // contexts so that later registrations cannot change how their shares are validated.
        if let Some(channel_jobs) = self.negotiated_jobs.get(&m.channel_id) {
            let referenced_job = channel_jobs
                .get(&m.job_id)
                .ok_or(Error::ShareDoNotMatchAnyJob)?;
            let merkle_path = referenced_job.merkle_path.clone().into_inner();
            let extended_job = job_creator::extended_job_from_custom_job(
                referenced_job,
                additional_coinbase_script_data.len() as u8,
                self.full_extranonce_len(m.channel_id)? as u8,
            )?;
            let prev_blockhash = crate::utils::u256_to_block_hash(referenced_job.prev_hash.clone());
            let bits = referenced_job.nbits;
            match self.inner.check_target(
                Share::Extended(m.clone()),
                target,
                None,
                0,
                merkle_path,
                extended_job.coinbase_tx_prefix.as_ref(),
                extended_job.coinbase_tx_suffix.as_ref(),
                prev_blockhash,
                bits,
                Some(&additional_coinbase_script_data),
                referenced_job.version,
                true,
            ) {
                // Since this is a share for a custom job and there is no way to know if the share
                // do not met target cause pool sent a new extranonce prefix and the miner is still
                // using the old one we check also against the old one since we don't want to fail
                // in that case.
                Ok(OnNewShare::SendErrorDownstream(m_)) => {
                    match self.get_old_additional_coinbase_script_data(m.channel_id) {
                        Some(additional_coinbase_script_data) => {
                            let target = self.job_creator.last_target();
                            let merkle_path = referenced_job.merkle_path.clone().into_inner();
                            let extended_job = job_creator::extended_job_from_custom_job(
                                referenced_job,
                                additional_coinbase_script_data.len() as u8,
                                self.full_extranonce_len(m.channel_id)? as u8,
                            )?;
                            self.inner.check_target(
                                Share::Extended(m),
                                target,
                                None,
                                0,
                                merkle_path,
                                extended_job.coinbase_tx_prefix.as_ref(),
                                extended_job.coinbase_tx_suffix.as_ref(),
                                prev_blockhash,
                                bits,
                                Some(&additional_coinbase_script_data),
                                referenced_job.version,
                                true,
                            )
                        }
                        None => Ok(OnNewShare::SendErrorDownstream(m_)),
                    }
                }
                Ok(res) => Ok(res),
                Err(err) => Err(err),
            }
        } else {
            let mut referenced_job = self
                .inner
                .get_valid_job(m.job_id)
                .cloned()
                .ok_or(Error::ShareDoNotMatchAnyJob)?
                .0;
            let merkle_path = referenced_job.merkle_path.clone().into_inner();
            let template_id = self
                .job_creator
                .get_template_id_from_job(referenced_job.job_id)
                .ok_or(Error::NoTemplateForId)?;
            if let Some(suffix) = self
                .inner
                .extended_job_variants
                .get(&(m.channel_id, m.job_id))
            {
                referenced_job.coinbase_tx_suffix = suffix.clone().try_into()?;
            }
            // Normalize the marker after any caller-provided variant. A variant must
            // never erase the assigned identity of a channel sharing an extranonce prefix.
            if let Some(output) = self.channel_coinbase_output(m.channel_id) {
                output.apply(&mut referenced_job)?;
            }
            if let Some(output) = self
                .inner
                .extended_job_coinbase_outputs
                .get(&(m.channel_id, m.job_id))
            {
                append_coinbase_output(&mut referenced_job, output.clone())?;
            }
            let prev_blockhash = self
                .inner
                .last_prev_hash_
                .ok_or(Error::ShareDoNotMatchAnyJob)?;
            let bits = self
                .inner
                .last_prev_hash
                .as_ref()
                .ok_or(Error::ShareDoNotMatchAnyJob)?
                .0
                .nbits;
            self.inner.check_target(
                Share::Extended(m),
                target,
                Some(template_id),
                0,
                merkle_path,
                referenced_job.coinbase_tx_prefix.as_ref(),
                referenced_job.coinbase_tx_suffix.as_ref(),
                prev_blockhash,
                bits,
                Some(&additional_coinbase_script_data),
                referenced_job.version,
                referenced_job.version_rolling_allowed,
            )
        }
    }

    /// Reserves a group ID. Call [`Self::close_group`] to release an unused reservation.
    pub fn new_group_id(&mut self) -> u32 {
        let new_id = self.inner.ids.safe_lock(|ids| ids.new_group_id()).unwrap();
        self.inner.channel_to_group_id.insert(new_id, new_id);
        new_id
    }

    /// Utility function to return a new standard channel id
    pub fn new_standard_id_for_hom(&mut self) -> u32 {
        let hom_group_id = 0;
        let new_id = self
            .inner
            .ids
            .safe_lock(|ids| ids.new_channel_id(hom_group_id))
            .unwrap();
        new_id
    }

    /// Returns the full extranonce, extranonce1 (static for channel) + extranonce2 (miner nonce
    /// space)
    pub fn extranonce_from_downstream_extranonce(
        &self,
        ext: crate::mining::Extranonce,
    ) -> Option<crate::mining::Extranonce> {
        self.inner
            .extranonces
            .extranonce_from_downstream_extranonce(ext)
    }

    /// Inspect retained native metadata. For pool-created channels, the nonce prefix excludes
    /// separately stored additional coinbase script data.
    pub fn extended_channel(
        &self,
        channel_id: u32,
    ) -> Option<&OpenExtendedMiningChannelSuccessOwned> {
        self.inner.extended_channels.get(&channel_id)
    }

    /// Return the coinbase extranonce length recorded in this channel's native assignment.
    pub fn full_extranonce_len(&self, channel_id: u32) -> Result<usize, Error> {
        let channel = self
            .inner
            .extended_channels
            .get(&channel_id)
            .ok_or(Error::NotFoundChannelId)?;
        Ok(channel.extranonce_prefix.as_bytes().len() + usize::from(channel.extranonce_size))
    }

    /// Validate and cache complete native custom-job data. Token authorization remains caller policy.
    ///
    /// Retains at most [`MAX_CUSTOM_JOBS_PER_CHANNEL`] acknowledged jobs per channel. Registration
    /// evicts the oldest job at that limit; TP tip updates evict jobs with a different prev hash.
    /// Shares referencing an evicted job return [`Error::ShareDoNotMatchAnyJob`].
    pub fn on_new_set_custom_mining_job(
        &mut self,
        message: SetCustomMiningJobOwned,
    ) -> Result<SetCustomMiningJobSuccess, Error> {
        let extranonce_len = self.full_extranonce_len(message.channel_id)?;
        if let Some(output) = self.channel_coinbase_output(message.channel_id) {
            let outputs = job_creator::decode_outputs(message.coinbase_tx_outputs.as_bytes())?;
            let mut markers = outputs.iter().filter(|output| {
                output
                    .script_pubkey
                    .as_bytes()
                    .starts_with(b"\x6a\x1dCHSEQ")
            });
            if markers.next() != Some(&output.tx_out()) || markers.next().is_some() {
                return Err(Error::InvalidChannelCoinbaseOutput);
            }
        }
        let additional_len = self
            .get_additional_coinbase_script_data(message.channel_id, 0)
            .ok_or(Error::NotFoundChannelId)?
            .len();
        job_creator::extended_job_from_custom_job(
            &message,
            additional_len as u8,
            extranonce_len as u8,
        )?;
        let reply = SetCustomMiningJobSuccess {
            channel_id: message.channel_id,
            request_id: message.request_id,
            job_id: self.inner.job_ids.next(),
        };
        let channel_jobs = self.negotiated_jobs.entry(message.channel_id).or_default();
        if channel_jobs.len() >= MAX_CUSTOM_JOBS_PER_CHANNEL {
            // IDs are allocated monotonically, so the smallest ID is the oldest registration.
            // The scan is bounded and keeps share lookup in the existing hash map.
            if let Some(oldest_id) = channel_jobs.keys().min().copied() {
                channel_jobs.remove(&oldest_id);
            }
        }
        channel_jobs.insert(reply.job_id, message);
        Ok(reply)
    }

    /// Registers a custom job, converting validation failures into a native protocol rejection.
    /// The existing [`Self::on_new_set_custom_mining_job`] entrypoint retains its library errors.
    pub fn try_on_new_set_custom_mining_job(
        &mut self,
        message: SetCustomMiningJobOwned,
    ) -> Result<SetCustomMiningJobSuccess, SetCustomMiningJobErrorOwned> {
        let channel_id = message.channel_id;
        let request_id = message.request_id;
        self.on_new_set_custom_mining_job(message).map_err(|error| {
            let code = match error {
                Error::NotFoundChannelId => {
                    mining_sv2::ERROR_CODE_SET_CUSTOM_MINING_JOB_INVALID_CHANNEL_ID
                }
                Error::InvalidChannelCoinbaseOutput => {
                    mining_sv2::ERROR_CODE_SET_CUSTOM_MINING_JOB_INVALID_COINBASE_TX_OUTPUTS
                }
                Error::InvalidBip34Bytes(_) => {
                    mining_sv2::ERROR_CODE_SET_CUSTOM_MINING_JOB_INVALID_COINBASE_PREFIX
                }
                _ => mining_sv2::ERROR_CODE_SET_CUSTOM_MINING_JOB_INVALID_COINBASE_TX,
            };
            SetCustomMiningJobErrorOwned {
                channel_id,
                request_id,
                error_code: code.to_string().try_into().unwrap(),
            }
        })
    }

    /// Get extended channel ids
    pub fn get_extended_channels_ids(&self) -> Vec<u32> {
        self.inner.extended_channels.keys().copied().collect()
    }

    /// Update coinbase outputs
    pub fn update_pool_outputs(&mut self, outs: Vec<TxOut>) {
        self.pool_coinbase_outputs = outs;
    }

    /// Calls `ChannelFactory::update_target_for_channel`
    /// Set a particular downstream channel target.
    pub fn update_target_for_channel(
        &mut self,
        channel_id: u32,
        new_target: Target,
    ) -> Option<bool> {
        self.inner.update_target_for_channel(channel_id, new_target)
    }

    /// Set the target for this channel. This is the upstream target.
    pub fn set_target(&mut self, new_target: &mut Target) {
        self.inner.kind.set_target(new_target);
    }

    // TODO ret can not be larger then 32 bytes maybe use the stack for it?
    #[inline(always)]
    pub fn get_additional_coinbase_script_data(
        &self,
        channel_id: u32,
        job_id: u32,
    ) -> Option<Vec<u8>> {
        let channel_data = self
            .channel_to_additional_coinbase_script_data
            .get(&channel_id)?;
        debug_assert!({
            let have_old = self.job_ids_using_old_add_data.contains(&job_id);
            let not_have_old = channel_data.1.is_some();
            if have_old {
                !not_have_old
            } else {
                true
            }
        });
        match channel_data {
            (add_data, None) => Some(add_data.clone()),
            (add_data, Some(old_data)) => {
                if self.job_ids_using_old_add_data.contains(&job_id) {
                    Some(old_data.clone())
                } else {
                    Some(add_data.clone())
                }
            }
        }
    }

    // TODO ret can not be larger then 32 bytes maybe use the stack for it?
    #[inline(always)]
    fn get_old_additional_coinbase_script_data(&self, channel_id: u32) -> Option<Vec<u8>> {
        self.channel_to_additional_coinbase_script_data
            .get(&channel_id)?
            .1
            .clone()
    }

    /// This set a new additional coinbase script data for a particular channel. Think to keep in
    /// mind before using this function:
    /// 1. Standard hom channels are not affected by the change
    /// 2. The new additional data MUST have the exact same len as the additonal data used to
    ///    initialize the channle factory with PoolChannelFactory::new
    /// 3. For job provided by the pool, all the non future sent before the new additional data will
    ///    have the old additional data. All the future jobs and the non future jobs sent after the
    ///    new additional data will have the new additional data
    /// 4. Custom jobs will be checked against the new additional data, if the check fail we check
    ///    against the old additional data if also this check fail we return SubmitShareError
    pub fn change_additional_coinbase_script_data(
        &mut self,
        new_data: Vec<u8>,
        channel_id: u32,
    ) -> Result<MiningOwned, Error> {
        if self.additional_coinbase_script_data.len() == new_data.len() {
            let mut ids_for_old_data = HashSet::with_hasher(BuildNoHashHasher::default());
            for id in self.inner.id_to_job.keys() {
                ids_for_old_data.insert(*id);
            }
            self.job_ids_using_old_add_data = ids_for_old_data;
            match self
                .channel_to_additional_coinbase_script_data
                .get_mut(&channel_id)
            {
                Some(data) => {
                    data.1 = Some(data.0.clone());
                    data.0 = new_data.clone();
                    let res = SetExtranoncePrefixOwned {
                        channel_id,
                        extranonce_prefix: new_data.try_into().expect(""),
                    };
                    Ok(MiningOwned::SetExtranoncePrefix(res))
                }
                None => Err(Error::NotFoundChannelId),
            }
        } else {
            Err(Error::NewAdditionalCoinbaseDataLenDoNotMatch)
        }
    }

    pub fn close_channel(&mut self, channel_id: u32) {
        if self.inner.channel_to_group_id.get(&channel_id) == Some(&channel_id) {
            return;
        }
        self.channel_to_additional_coinbase_script_data
            .retain(|k, _| k != &channel_id);
        self.negotiated_jobs.remove(&channel_id);
        self.channel_coinbase_outputs.remove(&channel_id);
        self.user_identifier_to_channel
            .retain(|_, id| *id != channel_id);
        self.inner.close_channel(channel_id);
    }

    /// Closes grouped standard channels and releases an empty nonzero group reservation.
    /// Native extended members remain open; close them individually with [`Self::close_channel`].
    /// Group zero, individual channel IDs and unknown groups are ignored.
    pub fn close_group(&mut self, group_id: u32) {
        if group_id == 0 || self.inner.channel_to_group_id.get(&group_id) != Some(&group_id) {
            return;
        }
        let channel_ids: Vec<_> = self
            .inner
            .standard_channels_for_non_hom_downstreams
            .values()
            .filter(|channel| channel.group_id == group_id)
            .map(|channel| channel.channel_id)
            .collect();
        for channel_id in channel_ids {
            self.close_channel(channel_id);
        }
        self.inner.remove_empty_group(group_id);
    }

    /// Registers a channel-specific coinbase suffix for a pool-created extended job.
    pub fn register_extended_job_variant(
        &mut self,
        channel_id: u32,
        job_id: u32,
        coinbase_suffix: Vec<u8>,
    ) {
        self.inner
            .extended_job_variants
            .insert((channel_id, job_id), coinbase_suffix);
    }

    /// Registers one extra output for a pool-created job. It is appended after any full suffix
    /// variant and CHSEQ marker. Include this output
    /// in the job sent to the miner, and reserve its coinbase budget at the caller.
    pub fn register_extended_job_coinbase_output(
        &mut self,
        channel_id: u32,
        job_id: u32,
        output: TxOut,
    ) {
        self.inner
            .extended_job_coinbase_outputs
            .insert((channel_id, job_id), output);
    }

    /// Clones the canonical current extended job for a channel.
    ///
    /// Returns `None` once the channel selects custom work, even if its cached jobs have expired.
    pub fn current_extended_job_for_channel(
        &self,
        channel_id: u32,
    ) -> Option<NewExtendedMiningJobOwned> {
        if self.negotiated_jobs.contains_key(&channel_id) {
            return None;
        }

        if !self.inner.extended_channels.contains_key(&channel_id) {
            return None;
        }
        let mut job = self.inner.get_last_valid_job()?.0.clone();
        if let Some(output) = self.channel_coinbase_output(channel_id) {
            output.apply(&mut job).ok()?;
        }
        Some(job)
    }

    pub fn get_extranonce_len(&self) -> usize {
        self.inner.extranonces.get_len()
    }

    pub fn get_extranonce_prefix(&self, channel_id: u32) -> Option<Vec<u8>> {
        let channel = self.inner.extended_channels.get(&channel_id)?;
        Some(channel.extranonce_prefix.as_bytes().to_vec())
    }
}

/// Used by proxies that want to open extended channels with upstream. If the proxy has job
/// declaration capabilities, we set the job creator and the coinbase outs.
#[derive(Debug)]
pub struct ProxyExtendedChannelFactory {
    inner: ChannelFactory,
    job_creator: Option<JobsCreators>,
    pool_coinbase_outputs: Option<Vec<TxOut>>,
    // Id assigned to the extended channel by upstream
    upstream_channel: OpenExtendedMiningChannelSuccessOwned,
}

impl ProxyExtendedChannelFactory {
    /// Constructor
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ids: Arc<Mutex<GroupId>>,
        extranonces: ExtendedExtranonce,
        job_creator: Option<JobsCreators>,
        share_per_min: f32,
        kind: ExtendedChannelKind,
        pool_coinbase_outputs: Option<Vec<TxOut>>,
        upstream_channel: OpenExtendedMiningChannelSuccessOwned,
    ) -> Self {
        match &kind {
            ExtendedChannelKind::Proxy { .. } => {
                if job_creator.is_some() {
                    panic!("Channel factory of kind Proxy can not be initialized with a JobCreators");
                };
            },
            ExtendedChannelKind::ProxyJd { .. } => {
                if job_creator.is_none() {
                    panic!("Channel factory of kind ProxyJd must be initialized with a JobCreators");
                };
            }
            ExtendedChannelKind::Pool => panic!("Try to construct an ProxyExtendedChannelFactory with pool kind, kind must be Proxy or ProxyJd"),
        };
        let inner = ChannelFactory {
            ids,
            standard_channels_for_non_hom_downstreams: HashMap::with_hasher(
                BuildNoHashHasher::default(),
            ),
            standard_channels_for_hom_downstreams: HashMap::with_hasher(
                BuildNoHashHasher::default(),
            ),
            extended_channels: HashMap::with_hasher(BuildNoHashHasher::default()),
            extranonces,
            share_per_min,
            future_jobs: Vec::new(),
            last_prev_hash: None,
            last_prev_hash_: None,
            last_valid_jobs: [None, None, None],
            id_to_job: HashMap::with_hasher(BuildNoHashHasher::default()),
            extended_job_variants: HashMap::new(),
            extended_job_coinbase_outputs: HashMap::new(),
            added_elements: 0,
            kind,
            job_ids: Id::new(),
            channel_to_group_id: HashMap::with_hasher(BuildNoHashHasher::default()),
            future_templates: HashMap::with_hasher(BuildNoHashHasher::default()),
        };
        ProxyExtendedChannelFactory {
            inner,
            job_creator,
            pool_coinbase_outputs,
            upstream_channel,
        }
    }

    /// Calls `ChannelFactory::add_standard_channel`
    pub fn add_standard_channel(
        &mut self,
        request_id: u32,
        downstream_hash_rate: f32,
        id_header_only: bool,
        id: u32,
    ) -> Result<Vec<MiningOwned>, Error> {
        self.inner
            .add_standard_channel(request_id, downstream_hash_rate, id_header_only, id, None)
    }

    /// Calls `ChannelFactory::new_extended_channel`
    pub fn new_extended_channel(
        &mut self,
        request_id: u32,
        hash_rate: f32,
        min_extranonce_size: u16,
    ) -> Result<Vec<MiningOwned>, Error> {
        self.inner
            .new_extended_channel(request_id, hash_rate, min_extranonce_size, None)
            .map(|x| x.0)
    }

    /// Called only when a new prev hash is received by a Template Provider when job declaration is
    /// used. It matches the message with a `job_id`, creates a new custom job, and calls
    /// `ChannelFactory::on_new_prev_hash`
    pub fn on_new_prev_hash_from_tp(
        &mut self,
        m: &SetNewPrevHashFromTpOwned,
    ) -> Result<Option<(PartialSetCustomMiningJob, u32)>, Error> {
        if let Some(job_creator) = self.job_creator.as_mut() {
            let job_id = job_creator.on_new_prev_hash(m).unwrap_or(0);
            let new_prev_hash = StagedPhash {
                job_id,
                prev_hash: m.prev_hash.clone(),
                min_ntime: m.header_timestamp,
                nbits: m.n_bits,
            };
            let mut custom_job = None;
            if let Some(template) = self.inner.future_templates.get(&job_id) {
                custom_job = Some((
                    PartialSetCustomMiningJob {
                        version: template.version,
                        prev_hash: new_prev_hash.prev_hash.clone(),
                        min_ntime: new_prev_hash.min_ntime,
                        nbits: new_prev_hash.nbits,
                        coinbase_tx_version: template.coinbase_tx_version,
                        coinbase_prefix: template.coinbase_prefix.clone(),
                        coinbase_tx_input_n_sequence: template.coinbase_tx_input_sequence,
                        coinbase_tx_outputs: job_creator::coinbase_outputs_from_job(
                            &self
                                .inner
                                .future_jobs
                                .iter()
                                .find(|(job, _)| job.job_id == job_id)
                                .ok_or(Error::NoValidJob)?
                                .0,
                            self.inner.extranonces.get_len(),
                        )?,
                        coinbase_tx_locktime: template.coinbase_tx_locktime,
                        merkle_path: template.merkle_path.clone(),
                        future_job: template.future_template,
                    },
                    job_id,
                ));
            }
            self.inner.future_templates = HashMap::with_hasher(BuildNoHashHasher::default());
            self.inner.on_new_prev_hash(new_prev_hash)?;
            Ok(custom_job)
        } else {
            panic!("A channel factory without job creator do not have declaration capabilities")
        }
    }

    /// Called only when a new template is received by a Template Provider when job declaration is
    /// used. It creates a new custom job and calls
    /// `ChannelFactory::on_new_extended_mining_job`
    #[allow(clippy::type_complexity)]
    pub fn on_new_template(
        &mut self,
        m: &mut NewTemplateOwned,
    ) -> Result<
        (
            // downstream job_id -> downstream message (newextjob or newjob)
            HashMap<u32, MiningOwned, BuildNoHashHasher<u32>>,
            // PartialSetCustomMiningJob to send to the pool
            Option<PartialSetCustomMiningJob>,
            // job_id registered in the channel, the one that SetNewPrevHash refer to (upstsream
            // job id)
            u32,
        ),
        Error,
    > {
        if let (Some(job_creator), Some(pool_coinbase_outputs)) = (
            self.job_creator.as_mut(),
            self.pool_coinbase_outputs.as_mut(),
        ) {
            let new_job = job_creator.on_new_template(m, true, pool_coinbase_outputs.clone(), 0)?;
            let id = new_job.job_id;
            if !new_job.is_future() && self.inner.last_prev_hash.is_some() {
                let prev_hash = self.last_prev_hash().unwrap();
                let min_ntime = self.last_min_ntime().unwrap();
                let nbits = self.last_nbits().unwrap();
                let custom_mining_job = PartialSetCustomMiningJob {
                    version: m.version,
                    prev_hash,
                    min_ntime,
                    nbits,
                    coinbase_tx_version: m.coinbase_tx_version,
                    coinbase_prefix: m.coinbase_prefix.clone(),
                    coinbase_tx_input_n_sequence: m.coinbase_tx_input_sequence,
                    coinbase_tx_outputs: job_creator::coinbase_outputs_from_job(
                        &new_job,
                        self.inner.extranonces.get_len(),
                    )?,
                    coinbase_tx_locktime: m.coinbase_tx_locktime,
                    merkle_path: m.merkle_path.clone(),
                    future_job: m.future_template,
                };
                return Ok((
                    self.inner.on_new_extended_mining_job(new_job, None)?,
                    Some(custom_mining_job),
                    id,
                ));
            } else if new_job.is_future() {
                self.inner
                    .future_templates
                    .insert(new_job.job_id, m.clone());
            }
            Ok((
                self.inner.on_new_extended_mining_job(new_job, None)?,
                None,
                id,
            ))
        } else {
            panic!("Either channel factory has no job creator or pool_coinbase_outputs are not yet set")
        }
    }

    /// Called when a `SubmitSharesStandard` message is received from the downstream. We check the
    /// shares against the channel's respective target and return `OnNewShare` to let us know if
    /// and where the shares should be relayed
    pub fn on_submit_shares_extended(
        &mut self,
        m: SubmitSharesExtendedOwned,
    ) -> Result<OnNewShare, Error> {
        let merkle_path = self
            .inner
            .get_valid_job(m.job_id)
            .as_ref()
            .ok_or(Error::ShareDoNotMatchAnyJob)?
            .0
            .merkle_path
            .clone()
            .into_inner();

        let referenced_job = self
            .inner
            .get_valid_job(m.job_id)
            .cloned()
            .ok_or(Error::ShareDoNotMatchAnyJob)?
            .0;

        if referenced_job.job_id != m.job_id {
            let error = SubmitSharesErrorOwned {
                channel_id: m.channel_id,
                sequence_number: m.sequence_number,
                // Infallible unwrap we already know the len of the error code (is a
                // static string)
                error_code: mining_sv2::ERROR_CODE_SUBMIT_SHARES_INVALID_JOB_ID
                    .to_string()
                    .try_into()
                    .unwrap(),
            };
            return Ok(OnNewShare::SendErrorDownstream(error));
        }

        if let Some(job_creator) = self.job_creator.as_mut() {
            let template_id = job_creator
                .get_template_id_from_job(referenced_job.job_id)
                .ok_or(Error::NoTemplateForId)?;
            let bitcoin_target = job_creator.last_target();
            let prev_blockhash = self
                .inner
                .last_prev_hash_
                .ok_or(Error::ShareDoNotMatchAnyJob)?;
            let bits = self
                .inner
                .last_prev_hash
                .as_ref()
                .ok_or(Error::ShareDoNotMatchAnyJob)?
                .0
                .nbits;
            self.inner.check_target(
                Share::Extended(m),
                bitcoin_target,
                Some(template_id),
                self.upstream_channel.channel_id,
                merkle_path,
                referenced_job.coinbase_tx_prefix.as_ref(),
                referenced_job.coinbase_tx_suffix.as_ref(),
                prev_blockhash,
                bits,
                None,
                referenced_job.version,
                referenced_job.version_rolling_allowed,
            )
        } else {
            let bitcoin_target = [0; 32];
            // if there is not job_creator is not proxy duty to check if target is below or above
            // bitcoin target so we set bitcoin_target = 0.
            let prev_blockhash = self
                .inner
                .last_prev_hash_
                .ok_or(Error::ShareDoNotMatchAnyJob)?;
            let bits = self
                .inner
                .last_prev_hash
                .as_ref()
                .ok_or(Error::ShareDoNotMatchAnyJob)?
                .0
                .nbits;
            self.inner.check_target(
                Share::Extended(m),
                bitcoin_target.into(),
                None,
                self.upstream_channel.channel_id,
                merkle_path,
                referenced_job.coinbase_tx_prefix.as_ref(),
                referenced_job.coinbase_tx_suffix.as_ref(),
                prev_blockhash,
                bits,
                None,
                referenced_job.version,
                referenced_job.version_rolling_allowed,
            )
        }
    }

    /// Called when a `SubmitSharesStandard` message is received from the Downstream. We check the
    /// shares against the channel's respective target and return `OnNewShare` to let us know if
    /// and where the shares should be relayed
    pub fn on_submit_shares_standard(
        &mut self,
        m: SubmitSharesStandard,
    ) -> Result<OnNewShare, Error> {
        let merkle_path = self
            .inner
            .get_valid_job(m.job_id)
            .as_ref()
            .ok_or(Error::ShareDoNotMatchAnyJob)?
            .0
            .merkle_path
            .clone()
            .into_inner();
        let referenced_job = self
            .inner
            .get_valid_job(m.job_id)
            .cloned()
            .ok_or(Error::ShareDoNotMatchAnyJob)?
            .0;
        match self.inner.channel_to_group_id.get(&m.channel_id) {
            Some(g_id) => {
                if let Some(job_creator) = self.job_creator.as_mut() {
                    let template_id = job_creator
                        .get_template_id_from_job(
                            self.inner
                                .get_valid_job(m.job_id)
                                .as_ref()
                                .unwrap()
                                .0
                                .job_id,
                        )
                        .ok_or(Error::NoTemplateForId)?;
                    let bitcoin_target = job_creator.last_target();
                    let prev_blockhash = self
                        .inner
                        .last_prev_hash_
                        .ok_or(Error::ShareDoNotMatchAnyJob)?;
                    let bits = self
                        .inner
                        .last_prev_hash
                        .as_ref()
                        .ok_or(Error::ShareDoNotMatchAnyJob)?
                        .0
                        .nbits;
                    self.inner.check_target(
                        Share::Standard((m, *g_id)),
                        bitcoin_target,
                        Some(template_id),
                        self.upstream_channel.channel_id,
                        merkle_path,
                        referenced_job.coinbase_tx_prefix.as_ref(),
                        referenced_job.coinbase_tx_suffix.as_ref(),
                        prev_blockhash,
                        bits,
                        None,
                        referenced_job.version,
                        referenced_job.version_rolling_allowed,
                    )
                } else {
                    let bitcoin_target = [0; 32];
                    let prev_blockhash = self
                        .inner
                        .last_prev_hash_
                        .ok_or(Error::ShareDoNotMatchAnyJob)?;
                    let bits = self
                        .inner
                        .last_prev_hash
                        .as_ref()
                        .ok_or(Error::ShareDoNotMatchAnyJob)?
                        .0
                        .nbits;
                    // if there is not job_creator is not proxy duty to check if target is below or
                    // above bitcoin target so we set bitcoin_target = 0.
                    self.inner.check_target(
                        Share::Standard((m, *g_id)),
                        bitcoin_target.into(),
                        None,
                        self.upstream_channel.channel_id,
                        merkle_path,
                        referenced_job.coinbase_tx_prefix.as_ref(),
                        referenced_job.coinbase_tx_suffix.as_ref(),
                        prev_blockhash,
                        bits,
                        None,
                        referenced_job.version,
                        referenced_job.version_rolling_allowed,
                    )
                }
            }
            None => {
                let err = SubmitSharesErrorOwned {
                    channel_id: m.channel_id,
                    sequence_number: m.sequence_number,
                    error_code: mining_sv2::ERROR_CODE_SUBMIT_SHARES_INVALID_CHANNEL_ID
                        .to_string()
                        .try_into()
                        .unwrap(),
                };
                Ok(OnNewShare::SendErrorDownstream(err))
            }
        }
    }

    /// The upstream's native extended-channel assignment, including its group and extranonce size.
    pub fn upstream_channel(&self) -> &OpenExtendedMiningChannelSuccessOwned {
        &self.upstream_channel
    }

    /// Apply an upstream group reassignment for this extended channel.
    pub fn set_group_channel(&mut self, message: &mining_sv2::SetGroupChannel<'_>) {
        if message
            .channel_ids
            .iter()
            .any(|id| *id == self.upstream_channel.channel_id)
        {
            self.upstream_channel.group_channel_id = message.group_channel_id;
        }
    }

    fn accepts_upstream_channel(&self, channel_id: u32) -> bool {
        channel_id == self.upstream_channel.channel_id
            || channel_id == self.upstream_channel.group_channel_id
    }

    /// Calls `ChannelFactory::on_new_prev_hash`
    pub fn on_new_prev_hash(&mut self, m: SetNewPrevHashOwned) -> Result<(), Error> {
        if !self.accepts_upstream_channel(m.channel_id) {
            return Err(Error::NotFoundChannelId);
        }
        self.inner.on_new_prev_hash(StagedPhash {
            job_id: m.job_id,
            prev_hash: m.prev_hash.clone(),
            min_ntime: m.min_ntime,
            nbits: m.nbits,
        })
    }

    /// Calls `ChannelFactory::on_new_extended_mining_job`
    pub fn on_new_extended_mining_job(
        &mut self,
        m: NewExtendedMiningJobOwned,
    ) -> Result<HashMap<u32, MiningOwned, BuildNoHashHasher<u32>>, Error> {
        if !self.accepts_upstream_channel(m.channel_id) {
            return Err(Error::NotFoundChannelId);
        }
        self.inner.on_new_extended_mining_job(m, None)
    }

    /// Set new target
    pub fn set_target(&mut self, new_target: &mut Target) {
        self.inner.kind.set_target(new_target);
    }

    /// Get last valid job version
    pub fn last_valid_job_version(&self) -> Option<u32> {
        self.inner
            .get_last_valid_job()
            .as_ref()
            .map(|j| j.0.version)
    }

    /// Return a mining job by its `job_id`.
    /// This can be `None` if we try to fetch a stale job because we clean up the valid jobs array
    /// upon receiving a new `PrevHash`
    pub fn job(&self, job_id: u32) -> Option<&(NewExtendedMiningJobOwned, Vec<u32>)> {
        self.inner.get_valid_job(job_id)
    }

    /// Returns the full extranonce, extranonce1 (static for channel) + extranonce2 (miner nonce
    /// space)
    pub fn extranonce_from_downstream_extranonce(
        &self,
        ext: crate::mining::Extranonce,
    ) -> Option<crate::mining::Extranonce> {
        self.inner
            .extranonces
            .extranonce_from_downstream_extranonce(ext)
    }

    /// Returns the most recent prev hash
    pub fn last_prev_hash(&self) -> Option<binary_sv2::U256Owned> {
        self.inner
            .last_prev_hash
            .as_ref()
            .map(|f| f.0.prev_hash.clone())
    }

    /// Get last min ntime
    pub fn last_min_ntime(&self) -> Option<u32> {
        self.inner.last_prev_hash.as_ref().map(|f| f.0.min_ntime)
    }

    /// Get last nbits
    pub fn last_nbits(&self) -> Option<u32> {
        self.inner.last_prev_hash.as_ref().map(|f| f.0.nbits)
    }

    /// Get extranonce_size
    pub fn extranonce_size(&self) -> usize {
        self.inner.extranonces.get_len()
    }

    /// Get extranonce_2 size
    pub fn channel_extranonce2_size(&self) -> usize {
        self.inner.extranonces.get_len() - self.inner.extranonces.get_range0_len()
    }

    // Only used when the proxy is using Job Declaration
    /// Updates pool outputs
    pub fn update_pool_outputs(&mut self, outs: Vec<TxOut>) {
        self.pool_coinbase_outputs = Some(outs);
    }

    /// Get this channel id
    pub fn get_this_channel_id(&self) -> u32 {
        self.upstream_channel.channel_id
    }

    /// Returns the extranonce1 len of the upstream. For a proxy, this would
    /// be the extranonce_prefix len
    pub fn get_upstream_extranonce1_len(&self) -> usize {
        self.inner.extranonces.get_range0_len()
    }

    /// Calls `ChannelFactory::update_target_for_channel`
    pub fn update_target_for_channel(
        &mut self,
        channel_id: u32,
        new_target: Target,
    ) -> Option<bool> {
        self.inner.update_target_for_channel(channel_id, new_target)
    }

    pub fn get_extranonce_len(&self) -> usize {
        self.inner.extranonces.get_len()
    }
}

/// Used by proxies for tracking upstream targets.
#[derive(Debug, Clone)]
pub enum ExtendedChannelKind {
    Proxy { upstream_target: Target },
    ProxyJd { upstream_target: Target },
    Pool,
}
impl ExtendedChannelKind {
    /// Set target
    pub fn set_target(&mut self, new_target: &mut Target) {
        match self {
            ExtendedChannelKind::Proxy { upstream_target }
            | ExtendedChannelKind::ProxyJd { upstream_target } => {
                std::mem::swap(upstream_target, new_target)
            }
            ExtendedChannelKind::Pool => warn!("Try to set upstream target for a pool"),
        }
    }
}
#[cfg(test)]
mod test {
    use super::*;
    use binary_sv2::{B064KOwned, Seq0255Owned, U256Owned};
    use bitcoin::{Amount, PublicKey, TxOut, WPubkeyHash};
    use mining_sv2::OpenStandardMiningChannelOwned;

    const BLOCK_REWARD: u64 = 2_000_000_000;

    // Block 1296 data
    // 01000000
    // c1397d4a33adeeb3383803e9ac3db4b2c2c9d6737cbabc13a534d24600000000
    // 89687b66140ac9874656270e066ed7ef81d5133ada2d0133f09322a87b161738
    // 4eb87749
    // ffff001d
    // 07cacb0e
    const _PUB_K: &str = "04c6d0969c2d98a5c19ba7c36c7937c5edbd60ff2a01397c4afe54f16cd641667ea0049ba6f9e1796ba3c8e49e1b504c532ebbaaa1010c3f7d9b83a8ea7fd800e2";
    const _BLOCK_HASH: &str = "000000009a4aed3e8ba7a978c6b50fea886fb496d66e696090a91d527200b002";
    const VERSION: u32 = 1;
    // version 01000000
    // inputs 01
    // prev out 0000000000000000000000000000000000000000000000000000000000000000ffffffff
    // script len 07
    // script 04ffff001d0177
    // sequence ffffffff
    // n inputs 01
    // amunt 00f2052a01000000
    // out lne 43
    // push 41
    // pub k 04c6d0969c2d98a5c19ba7c36c7937c5edbd60ff2a01397c4afe54f16cd641667ea0049ba6f9e1796ba3c8e49e1b504c532ebbaaa1010c3f7d9b83a8ea7fd800e2
    // checksig ac
    // locktime 00000000
    const COINBASE: &str = "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0177ffffffff0100f2052a01000000434104c6d0969c2d98a5c19ba7c36c7937c5edbd60ff2a01397c4afe54f16cd641667ea0049ba6f9e1796ba3c8e49e1b504c532ebbaaa1010c3f7d9b83a8ea7fd800e2ac00000000";
    const COINBASE_OUTPUT: &str = "4104c6d0969c2d98a5c19ba7c36c7937c5edbd60ff2a01397c4afe54f16cd641667ea0049ba6f9e1796ba3c8e49e1b504c532ebbaaa1010c3f7d9b83a8ea7fd800e2ac";
    const MERKLE_PATH: &str = "59bf8acbc9d60dfae841abecc3882b4181f2bdd8ac6c1d94001165ab3aef50b0";
    const NONCE: &str = "07cacb0e";
    const NTIME: &str = "4eb87749";

    // Prev  block data (1295)
    //01000000
    //cf578a234f330c287354e24234ff6b86d6ab9e4ddd3e5ba71a6bcbf600000000
    //72d12b99bdb63762bedc5db30bcffbd7903721bc736dd683de37b1a3632f9000
    //time: 2e8c7749 -> 49778c2e -> 1232571438
    //nbits: ffff001d -> 4294901789
    //29444816
    const PREV_HASH: &str = "0000000046d234a513bcba7c73d6c9c2b2b43dace9033838b3eead334a7d39c1";
    const PREV_HEADER_TIMESTAMP: u32 = 1232571438;
    const PREV_HEADER_NBITS: u32 = 486604799;

    fn _get_pub_key_hash() -> WPubkeyHash {
        let into_bin = decode_hex(_PUB_K).unwrap();
        let pk = PublicKey::from_slice(&into_bin[..]);
        let hash = pk.unwrap().pubkey_hash();
        WPubkeyHash::from_raw_hash(hash.to_raw_hash())
    }

    fn get_coinbase() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let parsed = decode_hex(COINBASE).unwrap();
        // Coinbase prefix in Sv2 is the bip34 block height in this tx there is no prefix
        let prefix = parsed[42..42].to_vec();
        let extranonce = parsed[42..49].to_vec();
        let suffix = parsed[49..].to_vec();
        (prefix, extranonce, suffix)
    }

    fn get_custom_job_outputs() -> B064KOwned {
        let out = TxOut {
            value: Amount::from_sat(BLOCK_REWARD),
            script_pubkey: decode_hex(COINBASE_OUTPUT).unwrap().into(),
        };
        bitcoin::consensus::serialize(&vec![out])
            .try_into()
            .unwrap()
    }

    fn get_merkle_path() -> Seq0255Owned<U256Owned> {
        let mut m_path = decode_hex(MERKLE_PATH).unwrap();
        m_path.reverse();
        let path: U256Owned = m_path.try_into().unwrap();
        vec![path].try_into().unwrap()
    }

    fn nbit_to_target(nbit: u32) -> U256Owned {
        let mut target = bitcoin::Target::from_compact(CompactTarget::from_consensus(nbit))
            .to_be_bytes()
            .to_vec();
        target.reverse();
        target.try_into().unwrap()
    }

    fn decode_hex(s: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
            .collect()
    }

    fn pool_channel_factory(
        additional_coinbase_script_data: Vec<u8>,
        extranonces: ExtendedExtranonce,
    ) -> PoolChannelFactory {
        let out = TxOut {
            value: Amount::from_sat(BLOCK_REWARD),
            script_pubkey: decode_hex(COINBASE_OUTPUT).unwrap().into(),
        };
        let extranonce_len = extranonces.get_len() as u8;
        PoolChannelFactory::new(
            Arc::new(Mutex::new(GroupId::new())),
            extranonces,
            JobsCreators::new(extranonce_len),
            1.0,
            ExtendedChannelKind::Pool,
            vec![out],
            additional_coinbase_script_data,
        )
        .unwrap()
    }

    fn new_template(template_id: u64) -> NewTemplateOwned {
        let (prefix, _, _) = get_coinbase();

        NewTemplateOwned {
            template_id,
            future_template: true,
            version: VERSION,
            coinbase_tx_version: 1,
            coinbase_prefix: prefix.try_into().unwrap(),
            coinbase_tx_input_sequence: u32::MAX,
            coinbase_tx_value_remaining: 5_000_000_000,
            coinbase_tx_outputs_count: 0,
            coinbase_tx_outputs: Vec::<u8>::new().try_into().unwrap(),
            coinbase_tx_locktime: 0,
            merkle_path: get_merkle_path(),
        }
    }

    fn new_prev_hash(template_id: u64) -> SetNewPrevHashFromTpOwned {
        let mut p_hash = decode_hex(PREV_HASH).unwrap();
        p_hash.reverse();

        SetNewPrevHashFromTpOwned {
            template_id,
            prev_hash: p_hash.try_into().unwrap(),
            header_timestamp: PREV_HEADER_TIMESTAMP,
            n_bits: PREV_HEADER_NBITS,
            target: nbit_to_target(PREV_HEADER_NBITS),
        }
    }

    fn custom_job_with_prefix(
        channel_id: u32,
        coinbase_prefix: Vec<u8>,
    ) -> SetCustomMiningJobOwned {
        let mut prev_hash = decode_hex(PREV_HASH).unwrap();
        prev_hash.reverse();

        SetCustomMiningJobOwned {
            channel_id,
            request_id: 1,
            token: Vec::<u8>::new().try_into().unwrap(),
            version: VERSION,
            prev_hash: prev_hash.try_into().unwrap(),
            min_ntime: PREV_HEADER_TIMESTAMP,
            nbits: PREV_HEADER_NBITS,
            coinbase_tx_version: 2,
            coinbase_prefix: coinbase_prefix.try_into().unwrap(),
            coinbase_tx_input_n_sequence: u32::MAX,
            coinbase_tx_outputs: get_custom_job_outputs(),
            coinbase_tx_locktime: 0,
            merkle_path: get_merkle_path(),
        }
    }

    fn submit_extended_share(channel_id: u32) -> SubmitSharesExtendedOwned {
        SubmitSharesExtendedOwned {
            channel_id,
            sequence_number: 1,
            job_id: 1,
            nonce: 0,
            ntime: PREV_HEADER_TIMESTAMP,
            version: VERSION,
            extranonce: vec![0; 8].try_into().unwrap(),
        }
    }

    #[test]
    fn replicate_pool_extended_channel_registers_prefix_and_additional_data() {
        let additional_coinbase_script_data = vec![1, 2, 3, 4, 5, 6];
        let extranonces = ExtendedExtranonce::new(0..0, 0..10, 10..16);
        let mut channel =
            pool_channel_factory(additional_coinbase_script_data.clone(), extranonces);
        let extranonce_prefix = vec![10, 11, 12, 13, 14, 15, 16, 17, 18, 19];
        let target = vec![255; 32].try_into().unwrap();
        let channel_id = 42;

        let result = channel
            .replicate_pool_extended_channel(
                7,
                target,
                extranonce_prefix.clone().try_into().unwrap(),
                channel_id,
                6,
                additional_coinbase_script_data.clone(),
            )
            .unwrap();

        let success = match &result[0] {
            MiningOwned::OpenExtendedMiningChannelSuccess(success) => success,
            _ => panic!(),
        };
        let mut downstream_extranonce_prefix = additional_coinbase_script_data.clone();
        downstream_extranonce_prefix.extend(extranonce_prefix.clone());

        assert_eq!(success.request_id, 7);
        assert_eq!(success.channel_id, channel_id);
        assert_eq!(success.extranonce_size, 6);
        assert_eq!(
            success.extranonce_prefix.clone().as_bytes().to_vec(),
            downstream_extranonce_prefix
        );
        assert!(channel.get_extended_channels_ids().contains(&channel_id));
        assert_eq!(
            channel.get_extranonce_prefix(channel_id).unwrap(),
            extranonce_prefix
        );
        assert_eq!(
            channel
                .get_additional_coinbase_script_data(channel_id, 1)
                .unwrap(),
            additional_coinbase_script_data
        );
    }

    #[test]
    fn replicate_pool_extended_channel_returns_current_job_and_prev_hash() {
        let additional_coinbase_script_data = vec![1, 2, 3, 4, 5, 6];
        let extranonces = ExtendedExtranonce::new(0..0, 0..10, 10..16);
        let mut channel =
            pool_channel_factory(additional_coinbase_script_data.clone(), extranonces);
        let template_id = 10;
        let mut template = new_template(template_id);
        channel.on_new_template(&mut template).unwrap();
        let prev_hash_from_tp = new_prev_hash(template_id);
        let current_job_id = channel
            .on_new_prev_hash_from_tp(&prev_hash_from_tp)
            .unwrap();

        let target = vec![255; 32].try_into().unwrap();
        let channel_id = 42;
        let result = channel
            .replicate_pool_extended_channel(
                7,
                target,
                vec![10, 11, 12, 13, 14, 15, 16, 17, 18, 19]
                    .try_into()
                    .unwrap(),
                channel_id,
                6,
                additional_coinbase_script_data,
            )
            .unwrap();

        assert_eq!(result.len(), 3);
        let success = match &result[0] {
            MiningOwned::OpenExtendedMiningChannelSuccess(success) => success,
            _ => panic!(),
        };
        let job = match &result[1] {
            MiningOwned::NewExtendedMiningJob(job) => job,
            _ => panic!(),
        };
        let prev_hash = match &result[2] {
            MiningOwned::SetNewPrevHash(prev_hash) => prev_hash,
            _ => panic!(),
        };

        assert_eq!(success.request_id, 7);
        assert_eq!(success.channel_id, channel_id);
        assert_eq!(job.channel_id, channel_id);
        assert_eq!(job.job_id, current_job_id);
        assert!(job.is_future());
        assert_eq!(prev_hash.channel_id, channel_id);
        assert_eq!(prev_hash.job_id, job.job_id);
        assert_eq!(prev_hash.min_ntime, PREV_HEADER_TIMESTAMP);
        assert_eq!(prev_hash.nbits, PREV_HEADER_NBITS);
    }

    #[test]
    fn replicate_pool_extended_channel_rejects_duplicate_prefix() {
        let additional_coinbase_script_data = vec![1, 2, 3, 4, 5, 6];
        let extranonces = ExtendedExtranonce::new(0..0, 0..10, 10..16);
        let mut channel =
            pool_channel_factory(additional_coinbase_script_data.clone(), extranonces);
        let extranonce_prefix = vec![10, 11, 12, 13, 14, 15, 16, 17, 18, 19];

        channel
            .replicate_pool_extended_channel(
                7,
                vec![255; 32].try_into().unwrap(),
                extranonce_prefix.clone().try_into().unwrap(),
                42,
                6,
                additional_coinbase_script_data.clone(),
            )
            .unwrap();

        let result = channel.replicate_pool_extended_channel(
            8,
            vec![255; 32].try_into().unwrap(),
            extranonce_prefix.try_into().unwrap(),
            43,
            6,
            additional_coinbase_script_data,
        );

        assert!(matches!(result, Err(Error::ExtranoncePrefixAlreadyInUse)));
        assert!(channel.get_extranonce_prefix(43).is_none());
    }

    #[test]
    fn new_extended_channel_skips_restored_prefix() {
        let additional_coinbase_script_data = vec![1, 2, 3, 4, 5, 6];
        let extranonces = ExtendedExtranonce::new(0..0, 0..10, 10..16);
        let mut channel =
            pool_channel_factory(additional_coinbase_script_data.clone(), extranonces);
        let restored_prefix = vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 1];

        channel
            .replicate_pool_extended_channel(
                7,
                vec![255; 32].try_into().unwrap(),
                restored_prefix.clone().try_into().unwrap(),
                42,
                6,
                additional_coinbase_script_data,
            )
            .unwrap();

        let result = channel
            .new_extended_channel(8, 100_000_000_000_000.0, 6)
            .unwrap();
        let fresh_channel_id = match &result[0] {
            MiningOwned::OpenExtendedMiningChannelSuccess(success) => success.channel_id,
            _ => panic!(),
        };

        assert_eq!(channel.get_extranonce_prefix(42).unwrap(), restored_prefix);
        assert_eq!(
            channel.get_extranonce_prefix(fresh_channel_id).unwrap(),
            vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 2]
        );
    }

    #[test]
    fn replicate_pool_extended_channel_rejects_additional_data_len_mismatch() {
        let extranonces = ExtendedExtranonce::new(0..0, 0..10, 10..16);
        let mut channel = pool_channel_factory(vec![1, 2, 3, 4, 5, 6], extranonces);
        let target = vec![255; 32].try_into().unwrap();
        let channel_id = 42;

        let result = channel.replicate_pool_extended_channel(
            7,
            target,
            vec![10, 11, 12, 13, 14, 15, 16, 17, 18, 19]
                .try_into()
                .unwrap(),
            channel_id,
            6,
            vec![1, 2, 3, 4, 5],
        );

        assert!(matches!(
            result,
            Err(Error::NewAdditionalCoinbaseDataLenDoNotMatch)
        ));
        assert!(channel.get_extranonce_prefix(channel_id).is_none());
    }

    #[test]
    fn replicate_pool_extended_channel_rejects_prefix_len_mismatch() {
        let extranonces = ExtendedExtranonce::new(0..0, 0..10, 10..16);
        let mut channel = pool_channel_factory(vec![1, 2, 3, 4, 5, 6], extranonces);
        let target = vec![255; 32].try_into().unwrap();
        let channel_id = 42;

        let result = channel.replicate_pool_extended_channel(
            7,
            target,
            vec![10, 11, 12, 13, 14, 15, 16, 17, 18].try_into().unwrap(),
            channel_id,
            6,
            vec![1, 2, 3, 4, 5, 6],
        );

        assert!(matches!(result, Err(Error::InvalidExtranonceSize(10, 9))));
        assert!(channel.get_extranonce_prefix(channel_id).is_none());
    }

    #[test]
    fn replicate_pool_extended_channel_rejects_extranonce_size_mismatch() {
        let extranonces = ExtendedExtranonce::new(0..0, 0..10, 10..16);
        let mut channel = pool_channel_factory(vec![1, 2, 3, 4, 5, 6], extranonces);
        let target = vec![255; 32].try_into().unwrap();
        let channel_id = 42;

        let result = channel.replicate_pool_extended_channel(
            7,
            target,
            vec![10, 11, 12, 13, 14, 15, 16, 17, 18, 19]
                .try_into()
                .unwrap(),
            channel_id,
            5,
            vec![1, 2, 3, 4, 5, 6],
        );

        assert!(matches!(result, Err(Error::InvalidExtranonceSize(6, 5))));
        assert!(channel.get_extranonce_prefix(channel_id).is_none());
    }

    #[test]
    fn test_complete_mining_round() {
        let (prefix, coinbase_extranonce, _) = get_coinbase();

        // Initialize a Channel of type Pool
        let out = TxOut {value: Amount::from_sat(BLOCK_REWARD), script_pubkey: decode_hex("4104c6d0969c2d98a5c19ba7c36c7937c5edbd60ff2a01397c4afe54f16cd641667ea0049ba6f9e1796ba3c8e49e1b504c532ebbaaa1010c3f7d9b83a8ea7fd800e2ac").unwrap().into()};
        let additional_coinbase_script_data = "".to_string();
        let creator = JobsCreators::new(7);
        let share_per_min = 1.0;
        // Create an ExtendedExtranonce of len 7:
        // upstream part is 0 bytes cause we are a pool so no more upstreams
        // self part is 7 bytes
        // downstream part is 0 cause in the test the downstream is HOM so we do not need to
        // reserve space for downstream
        let mut inner = coinbase_extranonce.clone();
        inner[6] = 0;
        let extranonces = ExtendedExtranonce::new_with_inner_only_test(0..0, 0..0, 0..7, inner);

        let ids = Arc::new(Mutex::new(GroupId::new()));
        let channel_kind = ExtendedChannelKind::Pool;
        let mut channel = PoolChannelFactory::new(
            ids,
            extranonces,
            creator,
            share_per_min,
            channel_kind,
            vec![out],
            additional_coinbase_script_data.into_bytes(),
        )
        .unwrap();

        // Build a NewTemplateOwned
        let new_template = NewTemplateOwned {
            template_id: 10,
            future_template: true,
            version: VERSION,
            coinbase_tx_version: 1,
            coinbase_prefix: prefix.try_into().unwrap(),
            coinbase_tx_input_sequence: u32::MAX,
            coinbase_tx_value_remaining: 5_000_000_000,
            coinbase_tx_outputs_count: 0,
            coinbase_tx_outputs: Vec::<u8>::new().try_into().unwrap(),
            coinbase_tx_locktime: 0,
            merkle_path: get_merkle_path(),
        };

        // "Send" the NewTemplateOwned to the channel
        let _ = channel.on_new_template(&mut (new_template.clone()));

        // Build a PrevHash
        let mut p_hash = decode_hex(PREV_HASH).unwrap();
        p_hash.reverse();
        let prev_hash = SetNewPrevHashFromTpOwned {
            template_id: 10,
            prev_hash: p_hash.try_into().unwrap(),
            header_timestamp: PREV_HEADER_TIMESTAMP,
            n_bits: PREV_HEADER_NBITS,
            target: nbit_to_target(PREV_HEADER_NBITS),
        };

        // "Send" the SetNewPrevHashOwned to channel
        let _ = channel.on_new_prev_hash_from_tp(&prev_hash);

        // Build open standard channel
        let open_standard_channel = OpenStandardMiningChannelOwned {
            request_id: 100,
            user_identity: "Gigi".to_string().try_into().unwrap(),
            nominal_hash_rate: 100_000_000_000_000.0,
            max_target: [255; 32].into(),
        };

        // "Send" the OpenStandardMiningChannelOwned to channel
        let result = loop {
            let id = channel.new_standard_id_for_hom();
            let result = channel
                .add_standard_channel(
                    open_standard_channel.get_request_id_as_u32(),
                    open_standard_channel.nominal_hash_rate,
                    true,
                    id,
                )
                .unwrap();
            let downsteram_extranonce = match &result[0] {
                MiningOwned::OpenStandardMiningChannelSuccess(msg) => {
                    msg.extranonce_prefix.clone().as_bytes().to_vec()
                }
                _ => panic!(),
            };
            if downsteram_extranonce == coinbase_extranonce {
                break result;
            }
        };
        let mut result = result.iter();

        // Get the expected job id and channel_id
        let mut channel_id = u32::MAX;
        let job_id = loop {
            match result.next().unwrap() {
                MiningOwned::OpenStandardMiningChannelSuccess(success) => {
                    channel_id = success.channel_id
                }
                MiningOwned::SetNewPrevHash(_) => (),
                MiningOwned::NewMiningJob(job) => break job.job_id,
                _ => panic!(),
            }
        };
        // make sure job management in channel factory is updated
        (0..job_id - 1).for_each(|_| {
            channel.job_creator.reset_new_templates(None);
            let _ = channel.on_new_template(&mut (new_template.clone()));
            let _ = channel.on_new_prev_hash_from_tp(&prev_hash);
        });

        // Build the success share
        let share = SubmitSharesStandard {
            channel_id,
            sequence_number: 2,
            job_id,
            nonce: u32::from_le_bytes(decode_hex(NONCE).unwrap().try_into().unwrap()),
            ntime: u32::from_le_bytes(decode_hex(NTIME).unwrap().try_into().unwrap()),
            version: 1,
        };

        // "Send" the Share to channel
        match channel.on_submit_shares_standard(share).unwrap() {
            OnNewShare::SendErrorDownstream(e) => panic!(
                "{:?} \n {}",
                e,
                std::str::from_utf8(&e.error_code.as_bytes().to_vec()[..]).unwrap()
            ),
            OnNewShare::SendSubmitShareUpstream(_) => panic!(),
            OnNewShare::RelaySubmitShareUpstream => panic!(),
            OnNewShare::ShareMeetBitcoinTarget(_) => (),
            OnNewShare::ShareMeetDownstreamTarget => panic!(),
        };
    }
    #[test]
    fn test_extranonce_prefix_in_hom() {
        let extranonce_prefix1 = [10, 11, 12];
        let (prefix, _, _) = get_coinbase();

        // Initialize a Channel of type Pool
        let out = TxOut {value: Amount::from_sat(BLOCK_REWARD), script_pubkey: decode_hex("4104c6d0969c2d98a5c19ba7c36c7937c5edbd60ff2a01397c4afe54f16cd641667ea0049ba6f9e1796ba3c8e49e1b504c532ebbaaa1010c3f7d9b83a8ea7fd800e2ac").unwrap().into()};
        let creator = JobsCreators::new(7);
        let share_per_min = 1.0;
        let extranonces = ExtendedExtranonce::new(0..0, 0..0, 0..7);

        let ids = Arc::new(Mutex::new(GroupId::new()));
        let channel_kind = ExtendedChannelKind::Pool;
        let mut channel = PoolChannelFactory::new(
            ids,
            extranonces,
            creator,
            share_per_min,
            channel_kind,
            vec![out],
            extranonce_prefix1.into(),
        )
        .unwrap();

        // Build a NewTemplateOwned
        let new_template = NewTemplateOwned {
            template_id: 10,
            future_template: true,
            version: VERSION,
            coinbase_tx_version: 1,
            coinbase_prefix: prefix.try_into().unwrap(),
            coinbase_tx_input_sequence: u32::MAX,
            coinbase_tx_value_remaining: 5_000_000_000,
            coinbase_tx_outputs_count: 0,
            coinbase_tx_outputs: Vec::<u8>::new().try_into().unwrap(),
            coinbase_tx_locktime: 0,
            merkle_path: get_merkle_path(),
        };

        // "Send" the NewTemplateOwned to the channel
        let _ = channel.on_new_template(&mut (new_template.clone()));

        // Build a PrevHash
        let mut p_hash = decode_hex(PREV_HASH).unwrap();
        p_hash.reverse();
        let prev_hash = SetNewPrevHashFromTpOwned {
            template_id: 10,
            prev_hash: p_hash.try_into().unwrap(),
            header_timestamp: PREV_HEADER_TIMESTAMP,
            n_bits: PREV_HEADER_NBITS,
            target: nbit_to_target(PREV_HEADER_NBITS),
        };

        // "Send" the SetNewPrevHashOwned to channel
        let _ = channel.on_new_prev_hash_from_tp(&prev_hash);

        let result = channel
            .add_standard_channel(100, 100_000_000_000_000.0, true, 2)
            .unwrap();
        let extranonce_prefix = match &result[0] {
            MiningOwned::OpenStandardMiningChannelSuccess(msg) => {
                msg.extranonce_prefix.clone().as_bytes().to_vec()
            }
            _ => panic!(),
        };
        assert!(extranonce_prefix[0..3] == extranonce_prefix1);
    }
    #[test]
    fn test_extranonce_prefix_in_extended() {
        let extranonce_prefix1 = [10, 11, 12];
        let extranonce_prefix2 = [14, 11, 12];
        let (prefix, _, _) = get_coinbase();

        // Initialize a Channel of type Pool
        let out = TxOut {value: Amount::from_sat(BLOCK_REWARD), script_pubkey: decode_hex("4104c6d0969c2d98a5c19ba7c36c7937c5edbd60ff2a01397c4afe54f16cd641667ea0049ba6f9e1796ba3c8e49e1b504c532ebbaaa1010c3f7d9b83a8ea7fd800e2ac").unwrap().into()};
        let creator = JobsCreators::new(16);
        let share_per_min = 1.0;
        let extranonces = ExtendedExtranonce::new(0..0, 0..8, 8..16);

        let ids = Arc::new(Mutex::new(GroupId::new()));
        let channel_kind = ExtendedChannelKind::Pool;
        let mut channel = PoolChannelFactory::new(
            ids,
            extranonces,
            creator,
            share_per_min,
            channel_kind,
            vec![out],
            extranonce_prefix1.into(),
        )
        .unwrap();

        // Build a NewTemplateOwned
        let new_template = NewTemplateOwned {
            template_id: 10,
            future_template: true,
            version: VERSION,
            coinbase_tx_version: 1,
            coinbase_prefix: prefix.try_into().unwrap(),
            coinbase_tx_input_sequence: u32::MAX,
            coinbase_tx_value_remaining: 5_000_000_000,
            coinbase_tx_outputs_count: 0,
            coinbase_tx_outputs: Vec::<u8>::new().try_into().unwrap(),
            coinbase_tx_locktime: 0,
            merkle_path: get_merkle_path(),
        };

        // "Send" the NewTemplateOwned to the channel
        let _ = channel.on_new_template(&mut (new_template.clone()));

        // Build a PrevHash
        let mut p_hash = decode_hex(PREV_HASH).unwrap();
        p_hash.reverse();
        let prev_hash = SetNewPrevHashFromTpOwned {
            template_id: 10,
            prev_hash: p_hash.try_into().unwrap(),
            header_timestamp: PREV_HEADER_TIMESTAMP,
            n_bits: PREV_HEADER_NBITS,
            target: nbit_to_target(PREV_HEADER_NBITS),
        };

        let _ = channel.on_new_prev_hash_from_tp(&prev_hash);

        let result = channel
            .new_extended_channel(100, 100_000_000_000_000.0, 2)
            .unwrap();
        let (extranonce_prefix, channel_id) = match &result[0] {
            MiningOwned::OpenExtendedMiningChannelSuccess(msg) => (
                msg.extranonce_prefix.clone().as_bytes().to_vec(),
                msg.channel_id,
            ),
            _ => panic!(),
        };
        assert!(extranonce_prefix[0..3] == extranonce_prefix1);
        match channel
            .change_additional_coinbase_script_data(extranonce_prefix2.to_vec(), channel_id)
        {
            Ok(MiningOwned::SetExtranoncePrefix(msg)) => {
                assert!(msg.extranonce_prefix.as_bytes()[0..3] == extranonce_prefix2);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn extended_fanout_rejects_non_future_template_without_mutating_job_creator() {
        let (prefix, _, _) = get_coinbase();
        let out = TxOut {value: Amount::from_sat(BLOCK_REWARD), script_pubkey: decode_hex("4104c6d0969c2d98a5c19ba7c36c7937c5edbd60ff2a01397c4afe54f16cd641667ea0049ba6f9e1796ba3c8e49e1b504c532ebbaaa1010c3f7d9b83a8ea7fd800e2ac").unwrap().into()};
        let creator = JobsCreators::new(7);
        let share_per_min = 1.0;
        let extranonces = ExtendedExtranonce::new(0..0, 0..0, 0..7);
        let ids = Arc::new(Mutex::new(GroupId::new()));
        let channel_kind = ExtendedChannelKind::Pool;
        let mut channel = PoolChannelFactory::new(
            ids,
            extranonces,
            creator,
            share_per_min,
            channel_kind,
            vec![out],
            Vec::new(),
        )
        .unwrap();

        let id = channel.new_standard_id_for_hom();
        channel
            .add_standard_channel(100, 100_000_000_000_000.0, true, id)
            .unwrap();
        let job_ids_before = channel.inner.job_ids.clone();

        let mut p_hash = decode_hex(PREV_HASH).unwrap();
        p_hash.reverse();
        let prev_hash = SetNewPrevHashFromTpOwned {
            template_id: 10,
            prev_hash: p_hash.try_into().unwrap(),
            header_timestamp: PREV_HEADER_TIMESTAMP,
            n_bits: PREV_HEADER_NBITS,
            target: nbit_to_target(PREV_HEADER_NBITS),
        };
        channel.job_creator.on_new_prev_hash(&prev_hash);

        let mut new_template = NewTemplateOwned {
            template_id: 10,
            future_template: false,
            version: VERSION,
            coinbase_tx_version: 1,
            coinbase_prefix: prefix.try_into().unwrap(),
            coinbase_tx_input_sequence: u32::MAX,
            coinbase_tx_value_remaining: 5_000_000_000,
            coinbase_tx_outputs_count: 0,
            coinbase_tx_outputs: Vec::<u8>::new().try_into().unwrap(),
            coinbase_tx_locktime: 0,
            merkle_path: get_merkle_path(),
        };

        assert!(matches!(
            channel.on_new_template_for_extended_fanout(&mut new_template),
            Err(Error::JobIsNotFutureButPrevHashNotPresent)
        ));
        assert!(channel.job_creator.get_template_id_from_job(1).is_none());
        assert_eq!(channel.inner.job_ids, job_ids_before);
    }

    #[test]
    fn extended_job_variants_are_selected_by_channel_and_job_id() {
        let extranonces = ExtendedExtranonce::new(0..0, 0..8, 8..16);
        let mut channel = pool_channel_factory(Vec::new(), extranonces);
        let channel_id_1 = match &channel
            .new_extended_channel(100, 100_000_000_000_000.0, 8)
            .unwrap()[0]
        {
            MiningOwned::OpenExtendedMiningChannelSuccess(success) => success.channel_id,
            _ => panic!(),
        };
        let channel_id_2 = match &channel
            .new_extended_channel(101, 100_000_000_000_000.0, 8)
            .unwrap()[0]
        {
            MiningOwned::OpenExtendedMiningChannelSuccess(success) => success.channel_id,
            _ => panic!(),
        };

        let template_id = 10;
        let mut template = new_template(template_id);
        let messages = channel.on_new_template(&mut template).unwrap();
        let job_1 = match messages.get(&channel_id_1).unwrap() {
            MiningOwned::NewExtendedMiningJob(job) => job.clone(),
            _ => panic!(),
        };
        let job_2 = match messages.get(&channel_id_2).unwrap() {
            MiningOwned::NewExtendedMiningJob(job) => job.clone(),
            _ => panic!(),
        };
        assert_eq!(job_1.job_id, job_2.job_id);

        let prefix = job_1.coinbase_tx_prefix.as_bytes().to_vec();
        let mut suffix_1 = job_1.coinbase_tx_suffix.as_bytes().to_vec();
        *suffix_1.last_mut().unwrap() = 2;
        let mut suffix_2 = job_2.coinbase_tx_suffix.as_bytes().to_vec();
        *suffix_2.last_mut().unwrap() = 3;
        let job_id = job_1.job_id;
        channel.register_extended_job_variant(channel_id_1, job_id, suffix_1.clone());
        channel.register_extended_job_variant(channel_id_2, job_id, suffix_2.clone());

        let mut prev_hash = new_prev_hash(template_id);
        prev_hash.target = vec![255; 32].try_into().unwrap();
        channel.on_new_prev_hash_from_tp(&prev_hash).unwrap();

        for (channel_id, suffix) in [(channel_id_1, suffix_1), (channel_id_2, suffix_2)] {
            let mut share = submit_extended_share(channel_id);
            share.job_id = job_id;
            let result = channel.on_submit_shares_extended(share).unwrap();
            let coinbase = match result {
                OnNewShare::ShareMeetBitcoinTarget((_, _, coinbase, _)) => coinbase,
                other => panic!("expected bitcoin-target share, got {:?}", other),
            };
            let expected_coinbase = [
                prefix.clone(),
                channel.get_extranonce_prefix(channel_id).unwrap(),
                vec![0; 8],
                suffix,
            ]
            .concat();
            assert_eq!(coinbase, expected_coinbase);
        }
    }

    #[test]
    fn current_extended_job_for_channel_returns_only_the_canonical_job() {
        let extranonces = ExtendedExtranonce::new(0..0, 0..8, 8..16);
        let mut channel = pool_channel_factory(Vec::new(), extranonces);
        let channel_id = match &channel
            .new_extended_channel(100, 100_000_000_000_000.0, 8)
            .unwrap()[0]
        {
            MiningOwned::OpenExtendedMiningChannelSuccess(success) => success.channel_id,
            _ => panic!(),
        };

        assert!(channel
            .current_extended_job_for_channel(channel_id)
            .is_none());

        let template_id = 10;
        let mut template = new_template(template_id);
        channel.on_new_template(&mut template).unwrap();
        channel
            .on_new_prev_hash_from_tp(&new_prev_hash(template_id))
            .unwrap();

        let canonical_job = channel.inner.get_last_valid_job().unwrap().0.clone();
        let mut variant_suffix = canonical_job.coinbase_tx_suffix.as_bytes().to_vec();
        *variant_suffix.last_mut().unwrap() ^= 1;
        channel.register_extended_job_variant(
            channel_id,
            canonical_job.job_id,
            variant_suffix.clone(),
        );

        let current_job = channel
            .current_extended_job_for_channel(channel_id)
            .unwrap();
        assert_eq!(current_job, canonical_job);
        assert_ne!(
            current_job.coinbase_tx_suffix.as_bytes().to_vec(),
            variant_suffix
        );
    }

    #[test]
    fn current_extended_job_for_channel_returns_none_for_custom_job() {
        let extranonces = ExtendedExtranonce::new(0..0, 0..8, 8..16);
        let mut channel = pool_channel_factory(Vec::new(), extranonces);
        let channel_id = match &channel
            .new_extended_channel(100, 100_000_000_000_000.0, 8)
            .unwrap()[0]
        {
            MiningOwned::OpenExtendedMiningChannelSuccess(success) => success.channel_id,
            _ => panic!(),
        };

        let template_id = 10;
        let mut template = new_template(template_id);
        channel.on_new_template(&mut template).unwrap();
        channel
            .on_new_prev_hash_from_tp(&new_prev_hash(template_id))
            .unwrap();
        assert!(channel
            .current_extended_job_for_channel(channel_id)
            .is_some());

        channel
            .on_new_set_custom_mining_job(custom_job_with_prefix(channel_id, vec![2, 0xe8, 3]))
            .unwrap();

        assert!(channel
            .current_extended_job_for_channel(channel_id)
            .is_none());
    }

    #[test]
    fn stale_extended_job_variant_cannot_make_evicted_base_job_valid() {
        let extranonces = ExtendedExtranonce::new(0..0, 0..8, 8..16);
        let mut channel = pool_channel_factory(Vec::new(), extranonces);
        let channel_id = match &channel
            .new_extended_channel(100, 100_000_000_000_000.0, 8)
            .unwrap()[0]
        {
            MiningOwned::OpenExtendedMiningChannelSuccess(success) => success.channel_id,
            _ => panic!(),
        };

        let template_id = 10;
        let mut template = new_template(template_id);
        let messages = channel.on_new_template(&mut template).unwrap();
        let stale_job = match messages.get(&channel_id).unwrap() {
            MiningOwned::NewExtendedMiningJob(job) => job.clone(),
            _ => panic!(),
        };
        let stale_job_id = stale_job.job_id;
        let stale_suffix = stale_job.coinbase_tx_suffix.as_bytes().to_vec();
        channel.register_extended_job_variant(channel_id, stale_job_id, stale_suffix.clone());
        channel
            .on_new_prev_hash_from_tp(&new_prev_hash(template_id))
            .unwrap();

        for template_id in 11..14 {
            let mut template = new_template(template_id);
            template.future_template = false;
            channel.on_new_template(&mut template).unwrap();
        }

        assert!(channel.inner.get_valid_job(stale_job_id).is_none());
        assert!(!channel
            .inner
            .extended_job_variants
            .contains_key(&(channel_id, stale_job_id)));

        channel.register_extended_job_variant(channel_id, stale_job_id, stale_suffix);
        assert!(channel
            .inner
            .extended_job_variants
            .contains_key(&(channel_id, stale_job_id)));
        let mut share = submit_extended_share(channel_id);
        share.job_id = stale_job_id;
        assert!(matches!(
            channel.on_submit_shares_extended(share),
            Err(Error::ShareDoNotMatchAnyJob)
        ));
    }

    #[test]
    fn malformed_custom_job_coinbase_prefix_returns_error_instead_of_panicking() {
        let out = TxOut {
            value: Amount::from_sat(BLOCK_REWARD),
            script_pubkey: decode_hex(
                "4104c6d0969c2d98a5c19ba7c36c7937c5edbd60ff2a01397c4afe54f16cd641667ea0049ba6f9e1796ba3c8e49e1b504c532ebbaaa1010c3f7d9b83a8ea7fd800e2ac",
            )
            .unwrap()
            .into(),
        };
        let creator = JobsCreators::new(16);
        let share_per_min = 1.0;
        let extranonces = ExtendedExtranonce::new(0..0, 0..8, 8..16);
        let ids = Arc::new(Mutex::new(GroupId::new()));
        let channel_kind = ExtendedChannelKind::Pool;
        let mut channel = PoolChannelFactory::new(
            ids,
            extranonces,
            creator,
            share_per_min,
            channel_kind,
            vec![out],
            Vec::new(),
        )
        .unwrap();

        let result = channel
            .new_extended_channel(100, 100_000_000_000_000.0, 2)
            .unwrap();
        let channel_id = match &result[0] {
            MiningOwned::OpenExtendedMiningChannelSuccess(msg) => msg.channel_id,
            _ => panic!(),
        };

        let malformed_prefix = vec![0x4c];
        channel
            .negotiated_jobs
            .entry(channel_id)
            .or_default()
            .insert(
                1,
                custom_job_with_prefix(channel_id, malformed_prefix.clone()),
            );

        assert!(matches!(
            channel.on_submit_shares_extended(submit_extended_share(channel_id)),
            Err(Error::InvalidBip34Bytes(bytes)) if bytes == malformed_prefix
        ));
        //panic!();
    }

    #[test]
    fn custom_job_cache_bounds_large_registrations_and_preserves_other_channels() {
        let mut factory =
            pool_channel_factory(Vec::new(), ExtendedExtranonce::new(0..0, 0..8, 8..16));
        let channel_ids = [1, 2].map(|request_id| {
            match &factory.new_extended_channel(request_id, 1_000., 8).unwrap()[0] {
                MiningOwned::OpenExtendedMiningChannelSuccess(channel) => channel.channel_id,
                other => panic!("expected open channel, got {other:?}"),
            }
        });
        let mut tip = new_prev_hash(1);
        tip.target = [255; 32].into();
        factory.on_new_prev_hash_from_tp(&tip).unwrap();
        let other_job = factory
            .on_new_set_custom_mining_job(custom_job_with_prefix(channel_ids[1], vec![2, 0xe8, 3]))
            .unwrap();
        let mut large_job = custom_job_with_prefix(channel_ids[0], vec![2, 0xe8, 3]);
        large_job.coinbase_tx_outputs = bitcoin::consensus::serialize(&vec![TxOut {
            value: Amount::from_sat(BLOCK_REWARD),
            script_pubkey: bitcoin::ScriptBuf::from_bytes(vec![0x51; 65_000]),
        }])
        .try_into()
        .unwrap();
        let mut ids = Vec::new();
        for request_id in 0..2_000 {
            large_job.request_id = request_id;
            ids.push(
                factory
                    .on_new_set_custom_mining_job(large_job.clone())
                    .unwrap()
                    .job_id,
            );
        }
        let retained = &factory.negotiated_jobs[&channel_ids[0]];
        assert_eq!(retained.len(), MAX_CUSTOM_JOBS_PER_CHANNEL);
        let retained_output_bytes: usize = retained
            .values()
            .map(|job| job.coinbase_tx_outputs.as_bytes().len())
            .sum();
        assert_eq!(
            retained_output_bytes,
            MAX_CUSTOM_JOBS_PER_CHANNEL * large_job.coinbase_tx_outputs.as_bytes().len()
        );
        assert_eq!(factory.negotiated_jobs[&channel_ids[1]].len(), 1);

        let first_retained = ids.len() - MAX_CUSTOM_JOBS_PER_CHANNEL;
        for job_id in [ids[0], ids[first_retained - 1]] {
            let mut share = submit_extended_share(channel_ids[0]);
            share.job_id = job_id;
            assert!(matches!(
                factory.on_submit_shares_extended(share),
                Err(Error::ShareDoNotMatchAnyJob)
            ));
        }
        // A malformed registration must not evict another acknowledged job.
        let mut invalid = large_job;
        invalid.coinbase_prefix = vec![0x4c].try_into().unwrap();
        assert!(factory.on_new_set_custom_mining_job(invalid).is_err());
        for (channel_id, job_id) in [
            (channel_ids[0], ids[first_retained]),
            (channel_ids[0], *ids.last().unwrap()),
            (channel_ids[1], other_job.job_id),
        ] {
            let mut share = submit_extended_share(channel_id);
            share.job_id = job_id;
            assert!(matches!(
                factory.on_submit_shares_extended(share).unwrap(),
                OnNewShare::ShareMeetBitcoinTarget(_)
            ));
        }
    }

    #[test]
    fn custom_job_cache_evicts_obsolete_tips_and_preserves_matching_jobs() {
        let mut factory =
            pool_channel_factory(Vec::new(), ExtendedExtranonce::new(0..0, 0..8, 8..16));
        let channel_id = match &factory.new_extended_channel(1, 1_000., 8).unwrap()[0] {
            MiningOwned::OpenExtendedMiningChannelSuccess(channel) => channel.channel_id,
            other => panic!("expected open channel, got {other:?}"),
        };
        let old = factory
            .on_new_set_custom_mining_job(custom_job_with_prefix(channel_id, vec![2, 0xe8, 3]))
            .unwrap();
        let mut incoming = custom_job_with_prefix(channel_id, vec![2, 0xe8, 3]);
        incoming.prev_hash = [6; 32].into();
        let new = factory.on_new_set_custom_mining_job(incoming).unwrap();
        let mut tip = new_prev_hash(1);
        tip.prev_hash = [6; 32].into();
        tip.target = [255; 32].into();
        for _ in 0..2 {
            factory.on_new_prev_hash_from_tp(&tip).unwrap();
            assert_eq!(factory.negotiated_jobs[&channel_id].len(), 1);
            let mut share = submit_extended_share(channel_id);
            share.job_id = old.job_id;
            assert!(matches!(
                factory.on_submit_shares_extended(share.clone()),
                Err(Error::ShareDoNotMatchAnyJob)
            ));
            share.job_id = new.job_id;
            assert!(matches!(
                factory.on_submit_shares_extended(share).unwrap(),
                OnNewShare::ShareMeetBitcoinTarget(_)
            ));
        }
        for next_tip in 7..17 {
            tip.prev_hash = [next_tip; 32].into();
            factory.on_new_prev_hash_from_tp(&tip).unwrap();
            assert!(factory.negotiated_jobs[&channel_id].is_empty());
        }
    }

    #[test]
    fn expired_custom_job_cannot_fall_back_to_a_colliding_canonical_job() {
        let mut factory =
            pool_channel_factory(Vec::new(), ExtendedExtranonce::new(0..0, 0..8, 8..16));
        let channel_id = match &factory.new_extended_channel(1, 1_000., 8).unwrap()[0] {
            MiningOwned::OpenExtendedMiningChannelSuccess(channel) => channel.channel_id,
            other => panic!("expected open channel, got {other:?}"),
        };
        let custom = factory
            .on_new_set_custom_mining_job(custom_job_with_prefix(channel_id, vec![2, 0xe8, 3]))
            .unwrap();
        factory.on_new_template(&mut new_template(1)).unwrap();
        let mut tip = new_prev_hash(1);
        tip.prev_hash = [6; 32].into();
        tip.target = [255; 32].into();
        let canonical_id = factory.on_new_prev_hash_from_tp(&tip).unwrap();
        assert_eq!(canonical_id, custom.job_id);
        assert!(factory.inner.get_valid_job(canonical_id).is_some());
        let mut share = submit_extended_share(channel_id);
        share.job_id = custom.job_id;
        assert!(matches!(
            factory.on_submit_shares_extended(share),
            Err(Error::ShareDoNotMatchAnyJob)
        ));
        assert!(factory
            .current_extended_job_for_channel(channel_id)
            .is_none());
        assert!(factory.negotiated_jobs[&channel_id].is_empty());
    }

    fn open_channel_for_state_test(
        factory: &mut PoolChannelFactory,
    ) -> OpenExtendedMiningChannelSuccessOwned {
        match factory
            .new_extended_channel(1, 1_000., 8)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
        {
            MiningOwned::OpenExtendedMiningChannelSuccess(channel) => channel,
            other => panic!("expected extended channel, got {other:?}"),
        }
    }

    #[test]
    fn channel_state_marker_counter_exhaustion_preserves_existing_work() {
        let mut factory =
            pool_channel_factory(Vec::new(), ExtendedExtranonce::new(0..0, 0..8, 8..16))
                .with_channel_coinbase_outputs();
        let mut sequence = [42; 24];
        sequence[16..].copy_from_slice(&(u64::MAX - 1).to_be_bytes());
        factory.last_channel_coinbase_sequence = Some(sequence);
        let channel = open_channel_for_state_test(&mut factory);
        let marker = factory.channel_coinbase_output(channel.channel_id).unwrap();
        assert_eq!(&marker.0[..16], &[42; 16]);
        assert_eq!(&marker.0[16..], &u64::MAX.to_be_bytes());
        factory
            .register_channel_user(channel.channel_id, b"live")
            .unwrap();
        let mut custom = custom_job_with_prefix(channel.channel_id, vec![2, 0xe8, 3]);
        let mut outputs =
            job_creator::decode_outputs(custom.coinbase_tx_outputs.as_bytes()).unwrap();
        outputs.push(marker.tx_out());
        custom.coinbase_tx_outputs = bitcoin::consensus::serialize(&outputs).try_into().unwrap();
        let accepted = factory
            .on_new_set_custom_mining_job(custom.clone())
            .unwrap();
        let allocated_ids = factory
            .inner
            .ids
            .safe_lock(|ids| format!("{ids:?}"))
            .unwrap();
        let groups = factory.inner.channel_to_group_id.clone();
        let mut restored = channel.clone();
        restored.channel_id = 99;
        restored.group_channel_id = 42;
        for _ in 0..2 {
            assert!(matches!(
                factory.new_extended_channel(2, 1_000., 8),
                Err(Error::ChannelCoinbaseSequenceExhausted)
            ));
            assert!(matches!(
                factory.restore_extended_channel(restored.clone(), vec![]),
                Err(Error::ChannelCoinbaseSequenceExhausted)
            ));
            assert_eq!(factory.last_channel_coinbase_sequence, Some(marker.0));
            assert_eq!(factory.extended_channel(channel.channel_id), Some(&channel));
            assert_eq!(
                factory.channel_coinbase_output(channel.channel_id),
                Some(marker)
            );
            assert_eq!(
                factory.channel_id_for_user(b"live"),
                Some(channel.channel_id)
            );
            assert_eq!(
                factory.negotiated_jobs[&channel.channel_id][&accepted.job_id],
                custom
            );
            assert_eq!(factory.inner.channel_to_group_id, groups);
            assert_eq!(
                factory
                    .inner
                    .ids
                    .safe_lock(|ids| format!("{ids:?}"))
                    .unwrap(),
                allocated_ids
            );
            assert!(factory.extended_channel(restored.channel_id).is_none());
        }
        factory.close_channel(channel.channel_id);
        assert!(matches!(
            factory.new_extended_channel(3, 1_000., 8),
            Err(Error::ChannelCoinbaseSequenceExhausted)
        ));
        assert_eq!(factory.last_channel_coinbase_sequence, Some(marker.0));
    }

    #[test]
    fn channel_state_invalid_restore_ids_do_not_change_live_channels_or_reservations() {
        let mut factory =
            pool_channel_factory(Vec::new(), ExtendedExtranonce::new(0..0, 0..8, 8..16))
                .with_channel_coinbase_outputs();
        let group = factory.new_group_id();
        let hom = factory.new_standard_id_for_hom();
        factory.add_standard_channel(1, 1_000., true, hom).unwrap();
        let extended = open_channel_for_state_test(&mut factory);
        let standard = match &factory
            .add_standard_channel(2, 1_000., false, group)
            .unwrap()[0]
        {
            MiningOwned::OpenStandardMiningChannelSuccess(channel) => channel.channel_id,
            other => panic!("expected standard channel, got {other:?}"),
        };
        factory
            .register_channel_user(extended.channel_id, b"live")
            .unwrap();
        let channels = factory.inner.extended_channels.clone();
        let groups = factory.inner.channel_to_group_id.clone();
        let additional = factory.channel_to_additional_coinbase_script_data.clone();
        let markers = factory.channel_coinbase_outputs.clone();
        let sequence = factory.last_channel_coinbase_sequence;
        let allocated_ids = factory
            .inner
            .ids
            .safe_lock(|ids| format!("{ids:?}"))
            .unwrap();
        for (channel_id, group_id, occupied_id) in [
            (0, 77, 0),
            (group, 77, group),
            (hom, 77, hom),
            (extended.channel_id, 77, extended.channel_id),
            (standard, 77, standard),
            (99, hom, hom),
            (99, extended.channel_id, extended.channel_id),
            (99, 99, 99),
        ] {
            let mut restored = extended.clone();
            restored.channel_id = channel_id;
            restored.group_channel_id = group_id;
            restored.extranonce_prefix = vec![9; 8].try_into().unwrap();
            assert!(matches!(
                factory.restore_extended_channel(restored, vec![]),
                Err(Error::ChannelIdAlreadyInUse(id)) if id == occupied_id
            ));
            assert_eq!(factory.inner.extended_channels, channels);
            assert_eq!(factory.inner.channel_to_group_id, groups);
            assert_eq!(
                factory.channel_to_additional_coinbase_script_data,
                additional
            );
            assert_eq!(factory.channel_coinbase_outputs, markers);
            assert_eq!(factory.last_channel_coinbase_sequence, sequence);
            assert_eq!(
                factory.channel_id_for_user(b"live"),
                Some(extended.channel_id)
            );
            assert_eq!(
                factory
                    .inner
                    .ids
                    .safe_lock(|ids| format!("{ids:?}"))
                    .unwrap(),
                allocated_ids
            );
            assert!(factory
                .inner
                .standard_channels_for_hom_downstreams
                .contains_key(&hom));
            assert!(factory
                .inner
                .standard_channels_for_non_hom_downstreams
                .contains_key(&GroupId::into_complete_id(group, standard)));
        }
        let mut restored = extended;
        restored.channel_id = u32::MAX;
        restored.group_channel_id = 77;
        restored.extranonce_prefix = vec![9; 8].try_into().unwrap();
        factory
            .restore_extended_channel(restored.clone(), vec![])
            .unwrap();
        assert_eq!(factory.extended_channel(u32::MAX), Some(&restored));
        assert_eq!(
            open_channel_for_state_test(&mut factory).channel_id,
            standard + 1
        );
        assert_eq!(factory.inner.channel_to_group_id[&77], 77);
    }

    #[test]
    fn channel_state_group_closure_cleans_delivery_tracking_and_preserves_other_members() {
        let mut factory =
            pool_channel_factory(vec![42], ExtendedExtranonce::new(0..0, 0..8, 8..16))
                .with_channel_coinbase_outputs();
        let group = factory.new_group_id();
        let other_group = factory.new_group_id();
        let channels = [group, group, other_group].map(|group_id| {
            match &factory
                .add_standard_channel(1, 1_000., false, group_id)
                .unwrap()[0]
            {
                MiningOwned::OpenStandardMiningChannelSuccess(channel) => channel.channel_id,
                other => panic!("expected standard channel, got {other:?}"),
            }
        });
        let hom = factory.new_standard_id_for_hom();
        factory.add_standard_channel(2, 1_000., true, hom).unwrap();
        let extended = open_channel_for_state_test(&mut factory);
        let extended = factory
            .extended_channel(extended.channel_id)
            .unwrap()
            .clone();
        factory
            .register_channel_user(extended.channel_id, b"live")
            .unwrap();
        factory.on_new_template(&mut new_template(10)).unwrap();
        factory
            .on_new_prev_hash_from_tp(&new_prev_hash(10))
            .unwrap();
        factory.on_new_template(&mut new_template(11)).unwrap();
        factory.close_channel(channels[0]);
        assert_eq!(factory.inner.channel_to_group_id[&group], group);
        for (_, sent_groups) in factory
            .inner
            .future_jobs
            .iter()
            .chain(factory.inner.last_valid_jobs.iter().flatten())
        {
            assert!(sent_groups.contains(&group));
            assert!(sent_groups.contains(&other_group));
        }
        factory.close_group(group);
        factory.close_group(group);
        for id in [group, channels[0], channels[1]] {
            assert!(!factory.inner.channel_to_group_id.contains_key(&id));
            assert!(!factory
                .channel_to_additional_coinbase_script_data
                .contains_key(&id));
        }
        assert_eq!(
            factory
                .inner
                .standard_channels_for_non_hom_downstreams
                .len(),
            1
        );
        assert_eq!(factory.inner.channel_to_group_id[&channels[2]], other_group);
        for (_, sent_groups) in factory
            .inner
            .future_jobs
            .iter()
            .chain(factory.inner.last_valid_jobs.iter().flatten())
        {
            assert!(!sent_groups.contains(&group));
            assert!(sent_groups.contains(&other_group));
        }
        let sent_groups = &factory.inner.last_prev_hash.as_ref().unwrap().1;
        assert!(!sent_groups.contains(&group));
        assert!(sent_groups.contains(&other_group));
        for ignored in [0, hom, extended.channel_id, u32::MAX] {
            factory.close_group(ignored);
        }
        let messages = factory.on_new_template(&mut new_template(12)).unwrap();
        assert!(!messages.contains_key(&group));
        for live in [other_group, hom, extended.channel_id] {
            assert!(messages.contains_key(&live));
        }
        assert_eq!(
            factory.extended_channel(extended.channel_id),
            Some(&extended)
        );
        assert_eq!(
            factory.channel_id_for_user(b"live"),
            Some(extended.channel_id)
        );
        assert!(factory
            .channel_coinbase_output(extended.channel_id)
            .is_some());
        let mappings = factory.inner.channel_to_group_id.clone();
        for _ in 0..100 {
            let empty_group = factory.new_group_id();
            factory.close_group(empty_group);
            factory.close_group(empty_group);
            assert_eq!(factory.inner.channel_to_group_id, mappings);
        }
    }

    #[test]
    fn channel_state_repeated_group_opens_and_closes_do_not_retain_channel_state() {
        let mut factory =
            pool_channel_factory(Vec::new(), ExtendedExtranonce::new(0..0, 0..8, 8..16));
        factory.on_new_template(&mut new_template(10)).unwrap();
        factory
            .on_new_prev_hash_from_tp(&new_prev_hash(10))
            .unwrap();
        factory.on_new_template(&mut new_template(11)).unwrap();
        let mut previous_id = 0;
        for _ in 0..1_000 {
            let group = factory.new_group_id();
            assert!(group > previous_id);
            let channel_id = match &factory
                .add_standard_channel(1, 1_000., false, group)
                .unwrap()[0]
            {
                MiningOwned::OpenStandardMiningChannelSuccess(channel) => channel.channel_id,
                other => panic!("expected standard channel, got {other:?}"),
            };
            assert!(channel_id > group);
            factory.close_channel(channel_id);
            assert!(factory.inner.channel_to_group_id.is_empty());
            assert!(factory
                .inner
                .standard_channels_for_non_hom_downstreams
                .is_empty());
            assert!(factory
                .channel_to_additional_coinbase_script_data
                .is_empty());
            for (_, groups) in factory
                .inner
                .future_jobs
                .iter()
                .chain(factory.inner.last_valid_jobs.iter().flatten())
            {
                assert!(groups.is_empty());
            }
            assert!(factory.inner.last_prev_hash.as_ref().unwrap().1.is_empty());
            previous_id = channel_id;
        }
        assert!(factory.new_group_id() > previous_id);
    }
}
