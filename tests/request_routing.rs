use common_messages_sv2::{Protocol, SetupConnection, SetupConnectionSuccess};
use framing_sv2::framing::{EncodableFrame, MessageFrame, SerializedFrame};
use mining_sv2::*;
use roles_logic_sv2::{
    common_properties::*,
    handlers::{mining::*, SendTo_},
    parsers::{Mining, MiningOwned},
    routing_logic::*,
    selectors::*,
    utils::{Id, Mutex},
    Error,
};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

#[derive(Debug, Default)]
struct Down {
    seen: Vec<u32>,
    extended: bool,
}
impl IsDownstream for Down {
    fn get_downstream_mining_data(&self) -> CommonDownstreamData {
        CommonDownstreamData {
            header_only: !self.extended,
            work_selection: false,
            version_rolling: false,
        }
    }
}
impl IsMiningDownstream for Down {}
type Selector = ProxyDownstreamMiningSelector<Down>;
type Router = MiningProxyRoutingLogic<Down, Up, Selector>;
#[derive(Debug)]
struct Up {
    mapper: RequestIdMapper,
    selector: Selector,
    channels: Vec<UpstreamChannel>,
    seen: Vec<u32>,
    channel_type: SupportedChannelTypes,
}
impl IsUpstream<Down, Selector> for Up {
    fn get_version(&self) -> u16 {
        2
    }
    fn get_flags(&self) -> u32 {
        1
    }
    fn get_supported_protocols(&self) -> Vec<Protocol> {
        vec![Protocol::MiningProtocol]
    }
    fn get_id(&self) -> u32 {
        1
    }
    fn get_mapper(&mut self) -> Option<&mut RequestIdMapper> {
        Some(&mut self.mapper)
    }
    fn get_remote_selector(&mut self) -> &mut Selector {
        &mut self.selector
    }
}
impl IsMiningUpstream<Down, Selector> for Up {
    fn total_hash_rate(&self) -> u64 {
        0
    }
    fn add_hash_rate(&mut self, _: u64) {}
    fn get_opened_channels(&mut self) -> &mut Vec<UpstreamChannel> {
        &mut self.channels
    }
    fn update_channels(&mut self, c: UpstreamChannel) {
        self.channels.push(c);
    }
}
impl<R: MiningRouter<Down, Up, Selector>> ParseDownstreamMiningMessages<Up, Selector, R> for Down {
    fn get_channel_type(&self) -> SupportedChannelTypes {
        if self.extended {
            SupportedChannelTypes::Extended
        } else {
            SupportedChannelTypes::Standard
        }
    }
    fn is_work_selection_enabled(&self) -> bool {
        false
    }
    fn handle_open_standard_mining_channel(
        &mut self,
        m: OpenStandardMiningChannel,
        up: Option<Arc<Mutex<Up>>>,
    ) -> Result<SendTo<Up>, Error> {
        self.seen.push(m.request_id);
        Ok(SendTo_::RelaySameMessageToRemote(up.unwrap()))
    }
    fn handle_open_extended_mining_channel(
        &mut self,
        m: OpenExtendedMiningChannel,
    ) -> Result<SendTo<Up>, Error> {
        self.seen.push(m.request_id);
        Ok(SendTo_::None(Some(MiningOwned::OpenExtendedMiningChannel(
            m.into_owned(),
        ))))
    }
    fn handle_update_channel(&mut self, _: UpdateChannel) -> Result<SendTo<Up>, Error> {
        unreachable!()
    }
    fn handle_submit_shares_standard(
        &mut self,
        _: SubmitSharesStandard,
    ) -> Result<SendTo<Up>, Error> {
        unreachable!()
    }
    fn handle_submit_shares_extended(
        &mut self,
        _: SubmitSharesExtended,
    ) -> Result<SendTo<Up>, Error> {
        unreachable!()
    }
    fn handle_set_custom_mining_job(&mut self, _: SetCustomMiningJob) -> Result<SendTo<Up>, Error> {
        unreachable!()
    }
}
impl<R: MiningRouter<Down, Up, Selector>> ParseUpstreamMiningMessages<Down, Selector, R> for Up {
    fn get_channel_type(&self) -> SupportedChannelTypes {
        self.channel_type
    }
    fn is_work_selection_enabled(&self) -> bool {
        false
    }
    fn handle_open_standard_mining_channel_success(
        &mut self,
        m: OpenStandardMiningChannelSuccess,
        remote: Option<Arc<Mutex<Down>>>,
    ) -> Result<SendTo<Down>, Error> {
        self.seen.push(m.request_id);
        Ok(SendTo_::RelaySameMessageToRemote(remote.unwrap()))
    }
    fn handle_open_mining_channel_error(
        &mut self,
        m: OpenMiningChannelError,
        remote: Option<Arc<Mutex<Down>>>,
    ) -> Result<SendTo<Down>, Error> {
        self.seen.push(m.request_id);
        match remote {
            Some(remote) => Ok(SendTo_::RelaySameMessageToRemote(remote)),
            None => Ok(SendTo_::None(Some(MiningOwned::OpenMiningChannelError(
                m.into_owned(),
            )))),
        }
    }
    fn handle_open_extended_mining_channel_success(
        &mut self,
        _: OpenExtendedMiningChannelSuccess,
    ) -> Result<SendTo<Down>, Error> {
        unreachable!()
    }
    fn handle_update_channel_error(
        &mut self,
        _: UpdateChannelError,
    ) -> Result<SendTo<Down>, Error> {
        unreachable!()
    }
    fn handle_close_channel(&mut self, _: CloseChannel) -> Result<SendTo<Down>, Error> {
        unreachable!()
    }
    fn handle_set_extranonce_prefix(
        &mut self,
        _: SetExtranoncePrefix,
    ) -> Result<SendTo<Down>, Error> {
        unreachable!()
    }
    fn handle_submit_shares_success(
        &mut self,
        _: SubmitSharesSuccess,
    ) -> Result<SendTo<Down>, Error> {
        unreachable!()
    }
    fn handle_submit_shares_error(&mut self, _: SubmitSharesError) -> Result<SendTo<Down>, Error> {
        unreachable!()
    }
    fn handle_new_mining_job(&mut self, _: NewMiningJob) -> Result<SendTo<Down>, Error> {
        unreachable!()
    }
    fn handle_new_extended_mining_job(
        &mut self,
        _: NewExtendedMiningJob,
    ) -> Result<SendTo<Down>, Error> {
        unreachable!()
    }
    fn handle_set_new_prev_hash(&mut self, _: SetNewPrevHash) -> Result<SendTo<Down>, Error> {
        unreachable!()
    }
    fn handle_set_custom_mining_job_success(
        &mut self,
        _: SetCustomMiningJobSuccess,
    ) -> Result<SendTo<Down>, Error> {
        unreachable!()
    }
    fn handle_set_custom_mining_job_error(
        &mut self,
        _: SetCustomMiningJobError,
    ) -> Result<SendTo<Down>, Error> {
        unreachable!()
    }
    fn handle_set_target(&mut self, _: SetTarget) -> Result<SendTo<Down>, Error> {
        unreachable!()
    }
}
fn encode(message: MiningOwned) -> Vec<u8> {
    let frame: MessageFrame<_> = message.try_into().unwrap();
    let mut bytes = vec![0; frame.encoded_length()];
    frame.encode_into(&mut bytes).unwrap();
    bytes
}
fn request() -> MiningOwned {
    MiningOwned::OpenStandardMiningChannel(OpenStandardMiningChannelOwned {
        request_id: 77,
        user_identity: b"miner".to_vec().try_into().unwrap(),
        nominal_hash_rate: 1000.,
        max_target: [255; 32].into(),
    })
}
fn success(id: u32) -> MiningOwned {
    MiningOwned::OpenStandardMiningChannelSuccess(OpenStandardMiningChannelSuccessOwned {
        request_id: id,
        channel_id: 100 + id,
        target: [255; 32].into(),
        extranonce_prefix: vec![1, 2].try_into().unwrap(),
        group_channel_id: 42,
    })
}
fn run_routing(
    encoded_fallback: bool,
    channel_type: SupportedChannelTypes,
    storage: &'static OnceLock<Mutex<Router>>,
) {
    let up = Arc::new(Mutex::new(Up {
        mapper: RequestIdMapper::new(),
        selector: Selector::new(),
        channels: vec![],
        seen: vec![],
        channel_type,
    }));
    let downstreams = [
        Arc::new(Mutex::new(Down::default())),
        Arc::new(Mutex::new(Down::default())),
    ];
    let data = downstreams[0]
        .safe_lock(|d| d.get_downstream_mining_data())
        .unwrap();
    let router = storage.get_or_init(|| {
        Mutex::new(Router {
            upstream_selector: GeneralMiningSelector::new(vec![up.clone()]),
            downstream_id_generator: Id::new(),
            downstream_to_upstream_map: HashMap::from([(data, vec![up.clone()])]),
        })
    });
    let route = || MiningRoutingLogic::Proxy(router);
    let mut ids = Vec::new();
    for down in &downstreams {
        let mut frame = SerializedFrame::from_bytes(encode(request())).unwrap();
        let forwarded = if encoded_fallback {
            let m = Mining::try_from((frame.header().msg_type(), frame.payload())).unwrap();
            let action =
                Down::handle_message_mining_deserialized(down.clone(), Ok(m), route()).unwrap();
            match action {
                SendTo_::RelayNewMessageToRemote(remote, m) => {
                    assert!(Arc::ptr_eq(&remote, &up));
                    encode(m)
                }
                _ => panic!("expected encoded relay"),
            }
        } else {
            let action = Down::handle_frame_mining(down.clone(), &mut frame, route()).unwrap();
            assert!(matches!(action, SendTo_::RelaySameMessageToRemote(_)));
            let mut bytes = vec![0; frame.encoded_length()];
            frame.encode_into(&mut bytes).unwrap();
            bytes
        };
        let mut forwarded = SerializedFrame::from_bytes(forwarded).unwrap();
        match Mining::try_from((forwarded.header().msg_type(), forwarded.payload())).unwrap() {
            Mining::OpenStandardMiningChannel(m) => {
                ids.push(m.request_id);
                assert_ne!(m.request_id, 77);
            }
            _ => unreachable!(),
        }
    }
    assert_ne!(ids[0], ids[1]);
    // Replies arrive in reverse order; one rejection exercises the same remapping guarantee.
    for index in [1, 0] {
        let reply = if index == 1 {
            success(ids[index])
        } else {
            MiningOwned::OpenMiningChannelError(OpenMiningChannelErrorOwned {
                request_id: ids[index],
                error_code: b"rejected".to_vec().try_into().unwrap(),
            })
        };
        let mut frame = SerializedFrame::from_bytes(encode(reply)).unwrap();
        let (remote, bytes) = if encoded_fallback {
            let m = Mining::try_from((frame.header().msg_type(), frame.payload())).unwrap();
            match Up::handle_message_mining_deserialized(up.clone(), Ok(m), route()).unwrap() {
                SendTo_::RelayNewMessageToRemote(r, m) => (r, encode(m)),
                _ => panic!("expected encoded relay"),
            }
        } else {
            let r = match Up::handle_frame_mining(up.clone(), &mut frame, route()).unwrap() {
                SendTo_::RelaySameMessageToRemote(r) => r,
                _ => panic!("expected direct relay"),
            };
            let mut bytes = vec![0; frame.encoded_length()];
            frame.encode_into(&mut bytes).unwrap();
            (r, bytes)
        };
        assert!(Arc::ptr_eq(&remote, &downstreams[index]));
        let mut forwarded = SerializedFrame::from_bytes(bytes).unwrap();
        match Mining::try_from((forwarded.header().msg_type(), forwarded.payload())).unwrap() {
            Mining::OpenStandardMiningChannelSuccess(m) => assert_eq!(m.request_id, 77),
            Mining::OpenMiningChannelError(m) => assert_eq!(m.request_id, 77),
            _ => unreachable!(),
        }
        assert!(up
            .safe_lock(|u| u.mapper.remove(ids[index]).is_none())
            .unwrap());
    }
    assert_eq!(up.safe_lock(|u| u.seen.clone()).unwrap(), vec![77, 77]);
    for (index, down) in downstreams.iter().enumerate() {
        assert_eq!(
            down.safe_lock(|d| d.seen.clone()).unwrap(),
            vec![ids[index]]
        );
    }
}
#[test]
fn direct_frames_route_duplicate_ids_and_reordered_replies() {
    static ROUTER: OnceLock<Mutex<Router>> = OnceLock::new();
    run_routing(false, SupportedChannelTypes::Standard, &ROUTER);
}
#[test]
fn decoded_messages_encode_remapped_native_replies() {
    static ROUTER: OnceLock<Mutex<Router>> = OnceLock::new();
    run_routing(true, SupportedChannelTypes::Standard, &ROUTER);
}

#[test]
fn mixed_connections_still_remap_standard_replies() {
    static ROUTERS: [OnceLock<Mutex<Router>>; 2] = [const { OnceLock::new() }; 2];
    for (encoded_fallback, storage) in [false, true].into_iter().zip(&ROUTERS) {
        run_routing(
            encoded_fallback,
            SupportedChannelTypes::GroupAndExtended,
            storage,
        );
    }
}

#[test]
fn extended_groups_preserve_assignments_and_change_membership() {
    let mut selector = Selector::new();
    let a = Arc::new(Mutex::new(Down::default()));
    let b = Arc::new(Mutex::new(Down::default()));
    for (id, remote) in [(10, a.clone()), (11, a.clone()), (12, b.clone())] {
        selector.on_open_standard_channel_request(id, remote.clone());
        let message = OpenExtendedMiningChannelSuccess {
            request_id: id,
            channel_id: id,
            target: (&[255; 32]).into(),
            extranonce_size: 8,
            extranonce_prefix: (&[1][..]).try_into().unwrap(),
            group_channel_id: 42,
        };
        assert!(Arc::ptr_eq(
            &selector.on_open_extended_channel_success(&message).unwrap(),
            &remote
        ));
    }
    assert_eq!(selector.get_downstreams_in_channel(42).unwrap().len(), 2);
    selector
        .set_group_channel(&SetGroupChannel {
            group_channel_id: 99,
            channel_ids: binary_sv2::Seq064K::new(vec![10, 12]).unwrap(),
        })
        .unwrap();
    assert_eq!(selector.get_downstreams_in_channel(42).unwrap().len(), 1);
    assert_eq!(selector.get_downstreams_in_channel(99).unwrap().len(), 2);
    assert!(selector
        .set_group_channel(&SetGroupChannel {
            group_channel_id: 1,
            channel_ids: binary_sv2::Seq064K::new(vec![11, 12345]).unwrap()
        })
        .is_err());
    assert_eq!(selector.get_downstreams_in_channel(42).unwrap().len(), 1);
    selector.remove_downstream(&a);
    assert!(selector.get_downstreams_in_channel(42).is_none());
    assert_eq!(selector.get_downstreams_in_channel(99).unwrap().len(), 1);
}

fn run_extended_rejection<R: MiningRouter<Down, Up, Selector>>(
    channel_type: SupportedChannelTypes,
    encoded_fallback: bool,
    storage: &'static OnceLock<Mutex<R>>,
    create_router: impl FnOnce(Arc<Mutex<Up>>) -> R,
) {
    let up = Arc::new(Mutex::new(Up {
        mapper: RequestIdMapper::new(),
        selector: Selector::new(),
        channels: vec![],
        seen: vec![],
        channel_type,
    }));
    let down = Arc::new(Mutex::new(Down {
        seen: vec![],
        extended: true,
    }));
    let router = storage.get_or_init(|| Mutex::new(create_router(up.clone())));
    let route = || MiningRoutingLogic::Proxy(router);
    let mut open = SerializedFrame::from_bytes(encode(MiningOwned::OpenExtendedMiningChannel(
        OpenExtendedMiningChannelOwned {
            request_id: 77,
            user_identity: b"miner".to_vec().try_into().unwrap(),
            nominal_hash_rate: 1000.,
            max_target: [255; 32].into(),
            min_extranonce_size: 8,
        },
    )))
    .unwrap();
    Down::handle_frame_mining(down.clone(), &mut open, route()).unwrap();
    assert_eq!(down.safe_lock(|d| d.seen.clone()).unwrap(), vec![77]);
    let wire = encode(MiningOwned::OpenMiningChannelError(
        OpenMiningChannelErrorOwned {
            request_id: 77,
            error_code: b"unsupported-extranonce-size".to_vec().try_into().unwrap(),
        },
    ));
    let mut frame = SerializedFrame::from_bytes(wire.clone()).unwrap();
    let result = if encoded_fallback {
        let decoded = Mining::try_from((frame.header().msg_type(), frame.payload())).unwrap();
        Up::handle_message_mining_deserialized(up.clone(), Ok(decoded), route())
    } else {
        Up::handle_frame_mining(up.clone(), &mut frame, route())
    }
    .unwrap();
    match result {
        SendTo_::None(Some(MiningOwned::OpenMiningChannelError(message))) => {
            assert_eq!(message.request_id, 77);
            assert_eq!(encode(MiningOwned::OpenMiningChannelError(message)), wire);
        }
        other => panic!("extended rejection did not reach its callback: {other:?}"),
    }
    assert_eq!(up.safe_lock(|u| u.seen.clone()).unwrap(), vec![77]);
}

#[test]
fn extended_rejections_reach_callback_with_proxy_routing() {
    static ROUTERS: [OnceLock<Mutex<Router>>; 4] = [const { OnceLock::new() }; 4];
    for ((channel_type, encoded_fallback), storage) in [
        (SupportedChannelTypes::Extended, false),
        (SupportedChannelTypes::Extended, true),
        (SupportedChannelTypes::GroupAndExtended, false),
        (SupportedChannelTypes::GroupAndExtended, true),
    ]
    .into_iter()
    .zip(&ROUTERS)
    {
        run_extended_rejection(channel_type, encoded_fallback, storage, |up| Router {
            upstream_selector: GeneralMiningSelector::new(vec![up]),
            downstream_id_generator: Id::new(),
            downstream_to_upstream_map: HashMap::new(),
        });
    }
}

#[derive(Debug)]
struct DefaultErrorRouter;

impl CommonRouter for DefaultErrorRouter {
    fn on_setup_connection(
        &mut self,
        _: &SetupConnection,
    ) -> Result<(CommonDownstreamData, SetupConnectionSuccess), Error> {
        unreachable!()
    }
}

// Deliberately inherit the public default for on_open_channel_error.
impl MiningRouter<Down, Up, Selector> for DefaultErrorRouter {
    fn on_open_standard_channel(
        &mut self,
        _: Arc<Mutex<Down>>,
        _: &mut OpenStandardMiningChannel,
        _: &CommonDownstreamData,
    ) -> Result<Arc<Mutex<Up>>, Error> {
        unreachable!()
    }

    fn on_open_standard_channel_success(
        &mut self,
        _: Arc<Mutex<Up>>,
        _: &mut OpenStandardMiningChannelSuccess,
    ) -> Result<Arc<Mutex<Down>>, Error> {
        unreachable!()
    }
}

#[test]
fn extended_rejections_reach_callback_with_custom_router_default() {
    static ROUTERS: [OnceLock<Mutex<DefaultErrorRouter>>; 4] = [const { OnceLock::new() }; 4];
    for ((channel_type, encoded_fallback), storage) in [
        (SupportedChannelTypes::Extended, false),
        (SupportedChannelTypes::Extended, true),
        (SupportedChannelTypes::GroupAndExtended, false),
        (SupportedChannelTypes::GroupAndExtended, true),
    ]
    .into_iter()
    .zip(&ROUTERS)
    {
        run_extended_rejection(channel_type, encoded_fallback, storage, |_| {
            DefaultErrorRouter
        });
    }
}

#[test]
fn mixed_connections_preserve_errors_for_mapped_requests_without_downstreams() {
    static ROUTERS: [OnceLock<Mutex<Router>>; 2] = [const { OnceLock::new() }; 2];
    for (encoded_fallback, storage) in [false, true].into_iter().zip(&ROUTERS) {
        let up = Arc::new(Mutex::new(Up {
            mapper: RequestIdMapper::new(),
            selector: Selector::new(),
            channels: vec![],
            seen: vec![],
            channel_type: SupportedChannelTypes::GroupAndExtended,
        }));
        let request_id = up.safe_lock(|u| u.mapper.on_open_channel(77)).unwrap();
        let router = storage.get_or_init(|| {
            Mutex::new(Router {
                upstream_selector: GeneralMiningSelector::new(vec![up.clone()]),
                downstream_id_generator: Id::new(),
                downstream_to_upstream_map: HashMap::new(),
            })
        });
        let mut frame = SerializedFrame::from_bytes(encode(MiningOwned::OpenMiningChannelError(
            OpenMiningChannelErrorOwned {
                request_id,
                error_code: b"rejected".to_vec().try_into().unwrap(),
            },
        )))
        .unwrap();
        let route = MiningRoutingLogic::Proxy(router);
        let result = if encoded_fallback {
            let decoded = Mining::try_from((frame.header().msg_type(), frame.payload())).unwrap();
            Up::handle_message_mining_deserialized(up.clone(), Ok(decoded), route)
        } else {
            Up::handle_frame_mining(up.clone(), &mut frame, route)
        };
        assert!(matches!(result, Err(Error::UnknownRequestId(id)) if id == request_id));
        assert!(up.safe_lock(|u| u.seen.is_empty()).unwrap());
    }
}

#[test]
fn unmapped_standard_rejections_still_report_an_error() {
    static ROUTERS: [OnceLock<Mutex<Router>>; 4] = [const { OnceLock::new() }; 4];
    for ((channel_type, encoded_fallback), storage) in [
        (SupportedChannelTypes::Standard, false),
        (SupportedChannelTypes::Standard, true),
        (SupportedChannelTypes::Group, false),
        (SupportedChannelTypes::Group, true),
    ]
    .into_iter()
    .zip(&ROUTERS)
    {
        let up = Arc::new(Mutex::new(Up {
            mapper: RequestIdMapper::new(),
            selector: Selector::new(),
            channels: vec![],
            seen: vec![],
            channel_type,
        }));
        let router = storage.get_or_init(|| {
            Mutex::new(Router {
                upstream_selector: GeneralMiningSelector::new(vec![up.clone()]),
                downstream_id_generator: Id::new(),
                downstream_to_upstream_map: HashMap::new(),
            })
        });
        let mut frame = SerializedFrame::from_bytes(encode(MiningOwned::OpenMiningChannelError(
            OpenMiningChannelErrorOwned {
                request_id: 77,
                error_code: b"rejected".to_vec().try_into().unwrap(),
            },
        )))
        .unwrap();
        let route = MiningRoutingLogic::Proxy(router);
        let result = if encoded_fallback {
            let decoded = Mining::try_from((frame.header().msg_type(), frame.payload())).unwrap();
            Up::handle_message_mining_deserialized(up.clone(), Ok(decoded), route)
        } else {
            Up::handle_frame_mining(up.clone(), &mut frame, route)
        };
        assert!(matches!(result, Err(Error::RequestIdNotMapped(77))));
        assert!(up.safe_lock(|u| u.seen.is_empty()).unwrap());
    }
}
