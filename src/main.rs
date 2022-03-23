use ::block::invalid::InvalidBlockErrorReason;
use block::block;
use blockchain::blockchain::Blockchain;
use claim::claim::Claim;
use clipboard::{ClipboardContext, ClipboardProvider};
use commands::command::Command;
use crossterm::{
    event::{self, Event as CEvent, KeyCode, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use events::events::{write_to_json, VrrbNetworkEvent};
use ledger::ledger::Ledger;
use log::info;
use messages::message_types::MessageType;
use miner::miner::Miner;
use network::components::StateComponent;
use commands::command::ComponentTypes;
use node::handler::{CommandHandler, MessageHandler};
use node::node::{Node, NodeAuth};
use public_ip;
use rand::Rng;
use reward::reward::{Category, RewardState};
use ritelinked::LinkedHashMap;
use simplelog::{Config, LevelFilter, WriteLogger};
use state::state::{Components, NetworkState};
use std::fs;
use std::time::{Duration, Instant};
use strum_macros::EnumIter;
use tokio::sync::mpsc;
use tui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans, Text},
    widgets::{Block, BorderType, Borders, ListState, Paragraph, Tabs},
    Terminal,
};
use tui_lib::helpers::*;
use txn::txn::Txn;
use unicode_width::UnicodeWidthStr;
use validator::validator::TxnValidator;
use wallet::wallet::WalletAccount;
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::channel;
use std::sync::mpsc::{Sender, Receiver};
use udp2p::protocol::protocol::packetize;
use udp2p::protocol::protocol::{AckMessage, Message, MessageKey, Header};
use udp2p::node::peer_id::PeerId;
use udp2p::node::peer_key::Key;
use udp2p::node::peer_info::PeerInfo;
use udp2p::gossip::protocol::GossipMessage;
use udp2p::discovery::kad::Kademlia;
use udp2p::discovery::routing::RoutingTable;
use udp2p::transport::transport::Transport;
use udp2p::transport::handler::MessageHandler as GossipMessageHandler;
use udp2p::gossip::gossip::{GossipConfig, GossipService};
use std::collections::{HashMap, HashSet};
use rand::thread_rng;
use std::env::args;
use udp2p::utils::utils::ByteRep;
use network::message;
use std::io::{Read, Write};
use std::net::{SocketAddrV4, SocketAddrV6};

pub const VALIDATOR_THRESHOLD: f64 = 0.60;

pub enum Event<I> {
    Input(I),
    Tick,
}

#[derive(Copy, Clone, EnumIter, Debug)]
pub enum MenuItem {
    Home,
    Wallet,
    Mining,
    Network,
    ChainData,
    Commands,
    HeaderChain,
    InvalidBlocks,
    ClaimMap,
    TxnPool,
    ClaimPool,
    PendingClaims,
    ConfirmedClaims,
    PendingTxns,
    ConfirmedTxns,
}

impl From<MenuItem> for usize {
    fn from(input: MenuItem) -> usize {
        match input {
            MenuItem::Home => 0,
            MenuItem::Wallet => 1,
            MenuItem::Mining => 2,
            MenuItem::Network => 3,
            MenuItem::ChainData => 4,
            MenuItem::Commands => 5,
            MenuItem::HeaderChain => 6,
            MenuItem::InvalidBlocks => 7,
            MenuItem::ClaimMap => 8,
            MenuItem::TxnPool => 9,
            MenuItem::ClaimPool => 10,
            MenuItem::PendingClaims => 11,
            MenuItem::ConfirmedClaims => 12,
            MenuItem::PendingTxns => 13,
            MenuItem::ConfirmedTxns => 14,
        }
    }
}

pub enum InputMode {
    Normal,
    Editing,
}

#[derive(Clone, Debug)]
pub struct App {
    pub miner: Option<Miner>,
    pub blockchain: Option<Blockchain>,
    pub wallet: Option<WalletAccount>,
    pub message_cache: LinkedHashMap<u128, MessageType>,
}

pub struct CommandInput {
    pub input: String,
    pub input_mode: InputMode,
    pub command_cache: Vec<String>,
}

impl Default for CommandInput {
    fn default() -> CommandInput {
        CommandInput {
            input: String::new(),
            input_mode: InputMode::Normal,
            command_cache: Vec::with_capacity(100),
        }
    }
}

impl Default for App {
    fn default() -> App {
        App {
            miner: None,
            blockchain: None,
            wallet: None,
            message_cache: LinkedHashMap::new(),
        }
    }
}

#[allow(unused_variables, unused_mut)]
#[async_std::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode().expect("can run in raw mode");
    let mut rng = rand::thread_rng();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tick_rate = tokio::time::Duration::from_millis(200);

    std::thread::spawn(move || {
        let mut last_tick = Instant::now();
        loop {
            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));

            if event::poll(timeout).expect("poll works") {
                if let CEvent::Key(key) = event::read().expect("can read events") {
                    if let Err(_) = tx.send(Event::Input(key)) {
                        info!("Can't send events");
                    }
                }
            }

            if last_tick.elapsed() > tick_rate {
                if let Ok(_) = tx.send(Event::Tick) {
                    last_tick = Instant::now();
                }
            }
        }
    });

    let stdout = std::io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let event_file_suffix: u8 = rng.gen();

    let directory = {
        if let Some(dir) = std::env::args().nth(2) {
            std::fs::create_dir_all(dir.clone())?;
            dir.clone()
        } else {
            std::fs::create_dir_all("./.vrrb_data".to_string())?;
            "./.vrrb_data".to_string()
        }
    };

    let menu_titles: Vec<_> = vec!["Home", "Wallet", "Mining", "Network", "ChainData"];
    let events_path = format!("{}/events_{}.json", directory.clone(), event_file_suffix);
    fs::File::create(events_path.clone()).unwrap();
    if let Err(_) = write_to_json(events_path.clone(), VrrbNetworkEvent::VrrbStarted) {
        info!("Error writting to json in main.rs 164");
    }
    let mut active_menu_item = MenuItem::Home;
    let mut wallet_list_state = ListState::default();
    let mut blockchain_fields = ListState::default();
    let mut last_hash_list_state = ListState::default();
    let mut invalid_block_list_state = ListState::default();
    let mut miner_fields = ListState::default();
    let mut claim_map_list_state = ListState::default();
    let mut txn_pool_list_state = ListState::default();
    let mut claim_pool_list_state = ListState::default();
    let mut txn_pool_status_list_state = ListState::default();
    let mut claim_pool_status_list_state = ListState::default();
    blockchain_fields.select(Some(0));
    wallet_list_state.select(Some(0));
    last_hash_list_state.select(Some(0));
    invalid_block_list_state.select(Some(0));
    miner_fields.select(Some(0));
    claim_map_list_state.select(Some(0));
    txn_pool_list_state.select(Some(0));
    claim_pool_list_state.select(Some(0));
    txn_pool_status_list_state.select(Some(0));
    claim_pool_status_list_state.select(Some(0));

    //____________________________________________________________________________________________________
    // Setup log file and db files

    let node_type = NodeAuth::Full;
    let log_file_suffix: u8 = rng.gen();
    let log_file_path = if let Some(path) = std::env::args().nth(4) {
        path
    } else {
        format!(
            "{}/vrrb_log_file_{}.log",
            directory.clone(),
            log_file_suffix
        )
    };
    let _ = WriteLogger::init(
        LevelFilter::Info,
        Config::default(),
        fs::File::create(log_file_path).unwrap(),
    );
    //____________________________________________________________________________________________________

    // ___________________________________________________________________________________________________
    // setup message and command sender/receiver channels for communication betwen various threads
    let (to_blockchain_sender, mut to_blockchain_receiver) = mpsc::unbounded_channel();
    let (to_miner_sender, mut to_miner_receiver) = mpsc::unbounded_channel();
    let (to_message_sender, mut to_message_receiver) = mpsc::unbounded_channel();
    let (from_message_sender, mut from_message_receiver) = mpsc::unbounded_channel();
    let (to_gossip_sender, mut to_gossip_receiver) = mpsc::unbounded_channel();
    let (command_sender, command_receiver) = mpsc::unbounded_channel();
    let (to_swarm_sender, mut to_swarm_receiver) = mpsc::unbounded_channel();
    let (to_state_sender, mut to_state_receiver) = mpsc::unbounded_channel();
    let (to_app_sender, mut to_app_receiver) = mpsc::unbounded_channel();
    let (to_transport_tx, to_transport_rx): (
        Sender<(SocketAddr, Message)>,
        Receiver<(SocketAddr, Message)>,
    ) = channel();
    let (to_gossip_tx, to_gossip_rx) = channel();
    let (to_kad_tx, to_kad_rx) = channel();
    let (incoming_ack_tx, incoming_ack_rx): (Sender<AckMessage>, Receiver<AckMessage>) = channel();
    let (to_app_tx, _to_app_rx) = channel::<GossipMessage>();

    //____________________________________________________________________________________________________

    let wallet = if let Some(secret_key) = std::env::args().nth(3) {
        WalletAccount::restore_from_private_key(secret_key)
    } else {
        WalletAccount::new()
    };

    let mut rng = rand::thread_rng();
    let file_suffix: u32 = rng.gen();
    let path = if let Some(path) = std::env::args().nth(5) {
        path
    } else {
        format!("{}/test_{}.json", directory.clone(), file_suffix)
    };

    let mut network_state = NetworkState::restore(&path);
    let ledger = Ledger::new();
    network_state.set_ledger(ledger.as_bytes());
    let reward_state = RewardState::start();
    network_state.set_reward_state(reward_state);

    //____________________________________________________________________________________________________
    // Node initialization
    let to_message_handler = MessageHandler::new(from_message_sender.clone(), to_message_receiver);
    let command_handler = CommandHandler::new(
        to_miner_sender.clone(),
        to_blockchain_sender.clone(),
        to_gossip_sender.clone(),
        to_swarm_sender.clone(),
        to_state_sender.clone(),
        to_gossip_tx.clone(),
        command_receiver,
    );

    let mut node = Node::new(node_type.clone(), command_handler, to_message_handler);
    let node_id = node.id.clone();
    let node_key = node.pubkey.clone();
    //____________________________________________________________________________________________________

    //____________________________________________________________________________________________________
    // Swarm initialization
    // Need to replace swarm with custom swarm-like struct.
    let pub_ip = public_ip::addr_v4().await;
    // let port: usize = thread_rng().gen_range(9292..19292);
    let port: usize = 19292;
    let addr = format!("{:?}:{:?}", pub_ip.clone().unwrap(), port.clone());
    let local_sock: std::net::SocketAddr = addr.parse().expect("unable to parse address");
    // Bind a UDP Socket to a Socket Address with a random port between
    // 9292 and 19292 on the localhost address.
    let sock: UdpSocket = UdpSocket::bind(format!("0.0.0.0:{:?}", port.clone())).expect("Unable to bind to address");



    // // Initialize local peer information
    let key: Key = Key::rand();
    let id: PeerId = PeerId::from_key(&key);
    let info: PeerInfo = PeerInfo::new(id, key, pub_ip.clone().unwrap(), port as u32);

    // // initialize a kademlia, transport and message handler instance
    let routing_table = RoutingTable::new(info.clone());
    let ping_pong = Instant::now();
    let interval = Duration::from_secs(20);
    let kad = Kademlia::new(routing_table, to_transport_tx.clone(), to_kad_rx, HashSet::new(), interval, ping_pong.clone());
    let mut transport = Transport::new(local_sock.clone(), incoming_ack_rx, to_transport_rx);
    let mut message_handler = GossipMessageHandler::new(
        to_transport_tx.clone(),
        incoming_ack_tx.clone(),
        HashMap::new(),
        to_kad_tx.clone(),
        to_gossip_tx.clone(),
    );
    let protocol_id = String::from("vrrb-0.1.0-test-net");
    let gossip_config = GossipConfig::new(
        protocol_id,
        8,
        3,
        8,
        3,
        12,
        3,
        0.4,
        Duration::from_millis(250),
        80,
    );
    let heartbeat = Instant::now();
    let ping_pong = Instant::now();
    let mut gossip = GossipService::new(
        local_sock.clone(),
        info.address.clone(),
        to_gossip_rx,
        to_transport_tx.clone(),
        to_app_tx.clone(),
        kad,
        gossip_config,
        heartbeat,
        ping_pong,
    );

    // // Inform the local node of their address (since the port is randomized)
    // //____________________________________________________________________________________________________

    // //____________________________________________________________________________________________________
    // // Dial peer if provided

    // //____________________________________________________________________________________________________

    // //____________________________________________________________________________________________________
    // // Swarm event thread
    // // Clone the socket for the transport and message handling thread(s)
    let thread_sock = sock.try_clone().expect("Unable to clone socket");
    let addr = local_sock.clone();
    std::thread::spawn(move || {
        let inner_sock = thread_sock.try_clone().expect("Unable to clone socket");
        std::thread::spawn(move || loop {
            transport.incoming_ack();
            transport.outgoing_msg(&inner_sock);
            transport.check_time_elapsed(&inner_sock);
        });

        loop {
            let local = addr.clone();
            let mut buf = [0u8; 655360];
            message_handler.recv_msg(&thread_sock, &mut buf, addr.clone());
        }
    });
    

    if let Some(to_dial) = args().nth(1) {
        let bootstrap: SocketAddr = to_dial.parse().expect("Unable to parse address");
        gossip.kad.bootstrap(&bootstrap);
        if let Some(bytes) = info.as_bytes() {
            gossip.kad.add_peer(bytes)
        }
    } else {
        if let Some(bytes) = info.as_bytes() {
            gossip.kad.add_peer(bytes)
        }
    }

    let thread_to_gossip = to_gossip_tx.clone();
    let (chat_tx, chat_rx) = channel::<GossipMessage>();
    let thread_node_id = node_id.clone();
    let msg_to_command_sender = command_sender.clone();
    std::thread::spawn(move || {
        loop {
            match chat_rx.recv() {
                Ok(gossip_msg) => {
                    if let Some(msg) = MessageType::from_bytes(&gossip_msg.data) {
                        if let Some(command) = message::process_message(msg, thread_node_id.clone(), addr.to_string()) {
                            if let Err(e) = msg_to_command_sender.send(command) {
                                info!("Error sending to command handler: {:?}", e);
                            }
                        }
                    }
                },
                Err(_) => {}
            }
        }
    });

    std::thread::spawn(move || {
        gossip.start(chat_tx.clone());
    });

    info!("Started gossip service");

    //____________________________________________________________________________________________________
    //____________________________________________________________________________________________________
    // Node thread
    tokio::task::spawn(async move {
        if let Err(_) = node.start().await {
            panic!("Unable to start node!")
        };
    });
    //____________________________________________________________________________________________________
    let mut command_input = CommandInput::default();
    let mut app = App::default();
    //____________________________________________________________________________________________________
    // Blockchain thread
    let mut blockchain_network_state = network_state.clone();
    let mut blockchain_reward_state = reward_state.clone();
    let blockchain_to_miner_sender = to_miner_sender.clone();
    let blockchain_to_swarm_sender = to_swarm_sender.clone();
    let blockchain_to_gossip_sender = to_gossip_tx.clone();
    let blockchain_to_blockchain_sender = to_blockchain_sender.clone();
    let blockchain_to_state_sender = to_state_sender.clone();
    let blockchain_to_app_sender = to_app_sender.clone();
    let blockchain_node_id = node_id.clone();
    std::thread::spawn(move || {
        let mut rng = rand::thread_rng();
        let file_suffix: u32 = rng.gen();
        let mut blockchain =
            Blockchain::new(&format!("{}/test_chain_{}.db", directory, file_suffix));
        if let Err(_) = blockchain_to_app_sender
            .send(Command::UpdateAppBlockchain(blockchain.clone().as_bytes()))
        {
            info!("Error sending blockchain update to App receiver.")
        }
        loop {
            let miner_sender = blockchain_to_miner_sender.clone();
            let swarm_sender = blockchain_to_swarm_sender.clone();
            let gossip_sender = blockchain_to_gossip_sender.clone();
            let state_sender = blockchain_to_state_sender.clone();
            let blockchain_sender = blockchain_to_blockchain_sender.clone();
            let app_sender = blockchain_to_app_sender.clone();
            // let blockchain_sender = blockchain_to_blockchain_sender.clone();
            if let Ok(command) = to_blockchain_receiver.try_recv() {
                match command {
                    Command::PendingBlock(block_bytes, sender_id) => {
                        let block = block::Block::from_bytes(&block_bytes);
                        if blockchain.updating_state {
                            blockchain
                                .future_blocks
                                .insert(block.clone().header.last_hash, block.clone());

                            if let Err(e) = app_sender
                                .send(Command::UpdateAppBlockchain(blockchain.clone().as_bytes()))
                            {
                                info!("Error sending Blockchain to app: {:?}", e);
                            }
                        } else {
                            if let Err(e) = blockchain.process_block(
                                &blockchain_network_state,
                                &blockchain_reward_state,
                                &block,
                            ) {
                                // TODO: Replace with Command::InvalidBlock being sent to the node or gossip
                                // and being processed.
                                // If the block is invalid because of BlockOutOfSequence Error request the missing blocks
                                // Or the current state of the network (first, missing blocks later)
                                // If the block is invalid because of a NotTallestChain Error tell the miner they are missing blocks.
                                // The miner should request the current state of the network and then all the blocks they are missing.
                                match e.details {
                                    InvalidBlockErrorReason::BlockOutOfSequence => {
                                        // Stash block in blockchain.future_blocks
                                        // Request state update once. Set "updating_state" field
                                        // in blockchain to true, so that it doesn't request it on
                                        // receipt of new future blocks which will also be invalid.
                                        blockchain.future_blocks.insert(block.header.last_hash.clone(), block.clone());
                                        if !blockchain.updating_state && !blockchain.processing_backlog {
                                            // send state request and set blockchain.updating state to true;
                                            info!("Error: {:?}", e);
                                            if let Some((_, v)) = blockchain.future_blocks.front() {
                                                let component = StateComponent::All;
                                                let message = MessageType::GetNetworkStateMessage {
                                                    sender_id: blockchain_node_id.clone(),
                                                    requested_from: sender_id.clone(),
                                                    requestor_address: addr.clone(),
                                                    requestor_node_type: node_type
                                                        .clone()
                                                        .as_bytes(),
                                                    lowest_block: v.header.block_height,
                                                    component: component.as_bytes(),
                                                };
                                                
                                                let msg_id = MessageKey::rand();
                                                let head = Header::Gossip;
                                                let gossip_msg = GossipMessage {
                                                    id: msg_id.inner(),
                                                    data: message.as_bytes(),
                                                    sender: addr.clone()
                                                };

                                                let msg = Message {
                                                    head,
                                                    msg: gossip_msg.as_bytes().unwrap()
                                                };

                                                std::thread::spawn(|| {
                                                    let listener = std::net::TcpListener::bind("0.0.0.0:19291").unwrap();
                                                    for stream in listener.incoming() {
                                                        match stream {
                                                            Ok(mut stream) => {
                                                                let mut buf = [0u8; 65536];
                                                                match stream.read(&mut buf) {
                                                                    Ok(size) => {
                                                                        info!("Received some bytes via stream: {:?}", size);
                                                                    }
                                                                    Err(_) => {
                                                                        info!("Error trying to receive some bytes");
                                                                    }
                                                                }
                                                            }
                                                            Err(e) => {}
                                                        }
                                                    }
                                                });
                                                
                                                info!("Requesting state update");
                                                if let Err(e) = gossip_sender
                                                    .send((addr.clone(), msg))
                                                {
                                                    info!("Error sending state update request to swarm sender: {:?}", e);
                                                };

                                                blockchain.updating_state = true;
                                                blockchain.started_updating = Some(udp2p::utils::utils::timestamp_now());
                                            }
                                        }
                                    }
                                    InvalidBlockErrorReason::NotTallestChain => {
                                        // Inform the miner they are missing blocks
                                        // info!("Error: {:?}", e);

                                    }
                                    _ => {
                                        if !blockchain.updating_state {
                                            let lowest_block = {
                                                if let Some(block) = blockchain.child.clone() {
                                                    block.clone()
                                                } else {
                                                    blockchain.genesis.clone().unwrap()
                                                }
                                            };
                                            info!("Error: {:?}", e);
                                            if block.header.block_height
                                                > lowest_block.header.block_height + 1
                                            {
                                                let component = StateComponent::All;
                                                let message = MessageType::GetNetworkStateMessage {
                                                    sender_id: blockchain_node_id.clone(),
                                                    requested_from: sender_id,
                                                    requestor_address: addr.clone(),
                                                    requestor_node_type: node_type
                                                        .clone()
                                                        .as_bytes(),
                                                    lowest_block: lowest_block.header.block_height,
                                                    component: component.as_bytes(),
                                                };

                                                let head = Header::Gossip;
                                                let msg_id = MessageKey::rand();
                                                let gossip_msg = GossipMessage {
                                                    id: msg_id.inner(),
                                                    data: message.as_bytes(),
                                                    sender: addr.clone()
                                                };
                                                let msg = Message {
                                                    head,
                                                    msg: gossip_msg.as_bytes().unwrap()
                                                };

                                                // TODO: Replace the below with sending to the correct channel
                                                if let Err(e) = gossip_sender
                                                    .send((addr.clone(), msg))
                                                {
                                                    info!("Error sending state update request to swarm sender: {:?}", e);
                                                };

                                                blockchain.updating_state = true;
                                            } else {
                                                // Miner is out of consensus tell them to update their state.
                                                let message = MessageType::InvalidBlockMessage {
                                                    block_height: block.header.block_height,
                                                    reason: e.details.as_bytes(),
                                                    miner_id: sender_id,
                                                    sender_id: blockchain_node_id.clone(),
                                                };

                                                let head = Header::Gossip;
                                                let msg_id = MessageKey::rand();
                                                let gossip_msg = GossipMessage {
                                                    id: msg_id.inner(),
                                                    data: message.as_bytes(),
                                                    sender: addr.clone()
                                                };
                                                let msg = Message {
                                                    head,
                                                    msg: gossip_msg.as_bytes().unwrap()
                                                };

                                                // TODO: Replace the below with sending to the correct channel
                                                if let Err(e) = gossip_sender
                                                    .send((addr.clone(), msg))
                                                {
                                                    info!("Error sending state update request to swarm sender: {:?}", e);
                                                };

                                                blockchain
                                                    .invalid
                                                    .insert(block.hash.clone(), block.clone());
                                            }
                                        }
                                    }
                                }

                                if let Err(_) = miner_sender
                                    .send(Command::InvalidBlock(block.clone().as_bytes()))
                                {
                                    info!("Error sending command to receiver");
                                };

                                if let Err(_) = app_sender.send(Command::UpdateAppBlockchain(
                                    blockchain.clone().as_bytes(),
                                )) {
                                    info!("Error sending updated blockchain to app");
                                }
                            } else {
                                blockchain_network_state.dump(
                                    &block.txns,
                                    block.header.block_reward.clone(),
                                    &block.claims,
                                    block.header.claim.clone(),
                                    &block.hash,
                                );
                                if let Err(_) = miner_sender
                                    .send(Command::ConfirmedBlock(block.clone().as_bytes()))
                                {
                                    info!("Error sending command to receiver");
                                }

                                if let Err(_) = miner_sender.send(Command::StateUpdateCompleted(
                                    blockchain_network_state.clone().as_bytes(),
                                )) {
                                    info!(
                                        "Error sending state update completed command to receiver"
                                    );
                                }

                                if let Err(_) = app_sender.send(Command::UpdateAppBlockchain(
                                    blockchain.clone().as_bytes(),
                                )) {
                                    info!("Error sending blockchain update to App receiver.")
                                }
                            }
                        }
                    }
                    Command::GetStateComponents(requestor, components_bytes, sender_id) => {
                        info!("Received request for State update");
                        let components = StateComponent::from_bytes(&components_bytes);
                        match components {
                            StateComponent::All => {
                                let genesis_bytes =
                                    if let Some(genesis) = blockchain.clone().genesis {
                                        Some(genesis.clone().as_bytes())
                                    } else {
                                        None
                                    };
                                let child_bytes = if let Some(block) = blockchain.clone().child {
                                    Some(block.clone().as_bytes())
                                } else {
                                    None
                                };
                                let parent_bytes = if let Some(block) = blockchain.clone().parent {
                                    Some(block.clone().as_bytes())
                                } else {
                                    None
                                };
                                let current_ledger = Some(
                                    blockchain_network_state.clone().db_to_ledger().as_bytes(),
                                );
                                let current_network_state =
                                    Some(blockchain_network_state.clone().as_bytes());
                                let components = Components {
                                    genesis: genesis_bytes,
                                    child: child_bytes,
                                    parent: parent_bytes,
                                    blockchain: None,
                                    ledger: current_ledger,
                                    network_state: current_network_state,
                                    archive: None,
                                };

                                if let Err(e) = state_sender.send(Command::RequestedComponents(
                                    requestor,
                                    components.as_bytes(),
                                    sender_id.clone(),
                                    blockchain_node_id.clone()
                                )) {
                                    info!(
                                        "Error sending requested components to state receiver: {:?}",
                                        e
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                    Command::StoreStateComponents(component_bytes, component_type) => {
                        if blockchain.updating_state {
                            blockchain.components_received.insert(component_type.clone());
                            match component_type {
                                ComponentTypes::Genesis => { 
                                    if let None = blockchain.check_missing_genesis() {
                                        blockchain.genesis = Some(block::Block::from_bytes(&component_bytes));
                                        info!("Stored genesis block");
                                    }
                                }
                                ComponentTypes::Child => { 
                                    if let None = blockchain.check_missing_child() {
                                        blockchain.child = Some(block::Block::from_bytes(&component_bytes));
                                        info!("Stored child block");
                                    }
                                }
                                ComponentTypes::Parent => { 
                                    if let None = blockchain.check_missing_parent() {
                                        blockchain.parent = Some(block::Block::from_bytes(&component_bytes));
                                        info!("Stored parent block"); 
                                    }
                                }
                                ComponentTypes::Ledger => {
                                    if let None = blockchain.check_missing_ledger() {
                                        let new_ledger = Ledger::from_bytes(component_bytes);
                                        blockchain_network_state.update_ledger(new_ledger);
                                        info!("Stored ledger");
                                    }
                                }
                                ComponentTypes::NetworkState => {
                                    if let None = blockchain.check_missing_state() {
                                        if let Ok(mut new_network_state) = NetworkState::from_bytes(component_bytes) {
                                            new_network_state.path = blockchain_network_state.path;
                                            blockchain_reward_state = new_network_state.reward_state.unwrap();
                                            blockchain_network_state = new_network_state;
                                            info!("Stored network state");
                                        }
                                    } 
                                }
                                _ => {}
                            }

                            if blockchain.received_core_components() {
                                info!("Received all core components");
                                blockchain.updating_state = false;
                                if let Err(e) = blockchain_sender.send(Command::ProcessBacklog) {
                                    info!("Error sending process backlog command to blockchain receiver: {:?}", e);
                                }
                                blockchain.processing_backlog = true;
                                if let Err(e) = app_sender
                                    .send(Command::UpdateAppBlockchain(blockchain.clone().as_bytes()))
                                {
                                    info!("Error sending updated blockchain to app: {:?}", e);
                                }
                            } else {
                                if blockchain.request_again() {
                                    let missing = blockchain.check_missing_components();
                                    info!("Missing Components: {:?}", missing);
                                    blockchain.components_received = HashSet::new();
                                }
                            }
                        }
                    }
                    Command::ProcessBacklog => {
                        if blockchain.processing_backlog {
                            let last_block = blockchain.clone().child.unwrap();
                            while let Some((_, block)) = blockchain.future_blocks.pop_front() {
                                if last_block.header.block_height >= block.header.block_height {
                                    info!("Block already processed, skipping")
                                } else {
                                    info!("Processing backlog block: {:?}", block.header.block_height);
                                    if let Err(e) = blockchain.process_block(
                                        &blockchain_network_state,
                                        &blockchain_reward_state,
                                        &block,
                                    ) {
                                        info!(
                                            "Error trying to process backlogged future blocks: {:?} -> {:?}",
                                            e,
                                            block,
                                        );
                                    } else {
                                        blockchain_network_state.dump(
                                            &block.txns,
                                            block.header.block_reward.clone(),
                                            &block.claims,
                                            block.header.claim.clone(),
                                            &block.hash,
                                        );
                                        info!("Processed and confirmed backlog block: {:?}", block.header.block_height);
                                        if let Err(e) = miner_sender
                                            .send(Command::ConfirmedBlock(block.clone().as_bytes()))
                                        {
                                            info!(
                                                "Error sending confirmed backlog block to miner: {:?}",
                                                e
                                            );
                                        }

                                        if let Err(e) = app_sender.send(Command::UpdateAppBlockchain(
                                            blockchain.clone().as_bytes(),
                                        )) {
                                            info!("Error sending blockchain to app: {:?}", e);
                                        }
                                    }
                                }
                            }
                            info!("Backlog processed");

                            if let Err(e) = miner_sender.send(Command::StateUpdateCompleted(
                                blockchain_network_state.clone().as_bytes(),
                            )) {
                                info!("Error sending updated network state to miner: {:?}", e);
                            }

                            if let Err(e) = app_sender
                                .send(Command::UpdateAppBlockchain(blockchain.clone().as_bytes()))
                            {
                                info!("Error sending updated blockchain to app: {:?}", e);
                            }
                            blockchain.processing_backlog = false;
                        }
                    }
                    Command::StateUpdateCompleted(network_state) => {
                        if let Ok(updated_network_state) = NetworkState::from_bytes(network_state) {
                            blockchain_network_state = updated_network_state;
                        } 
                        if let Err(e) = app_sender
                            .send(Command::UpdateAppBlockchain(blockchain.clone().as_bytes()))
                        {
                            info!("Error sending blockchain to app: {:?}", e);
                        }
                    }
                    Command::ClaimAbandoned(pubkey, claim_bytes) => {
                        let claim = Claim::from_bytes(&claim_bytes);
                        blockchain_network_state.abandoned_claim(claim.hash.clone());
                        if let Err(_) =
                            miner_sender.send(Command::ClaimAbandoned(pubkey, claim_bytes))
                        {
                            info!("Error sending claim abandoned command to miner");
                        }
                        if let Err(e) = miner_sender.send(Command::StateUpdateCompleted(
                            blockchain_network_state.clone().as_bytes(),
                        )) {
                            info!("Error sending updated network state to miner: {:?}", e);
                        }

                        if let Err(e) = app_sender
                            .send(Command::UpdateAppBlockchain(blockchain.clone().as_bytes()))
                        {
                            info!("Error sending blockchain to app: {:?}", e);
                        }
                    }
                    Command::SlashClaims(bad_validators) => {
                        blockchain_network_state.slash_claims(bad_validators);
                        if let Err(e) = app_sender
                            .send(Command::UpdateAppBlockchain(blockchain.clone().as_bytes()))
                        {
                            info!("Error sending blockchain to app: {:?}", e);
                        }
                    }
                    Command::NonceUp => {
                        blockchain_network_state.nonce_up();
                    }
                    Command::GetHeight => {
                        info!("Blockchain Height: {}", blockchain.chain.len());
                    }
                    _ => {}
                }
            }
        }
    });
    //____________________________________________________________________________________________________

    //____________________________________________________________________________________________________
    // Mining thread
    let mining_wallet = wallet.clone();
    let miner_network_state = network_state.clone();
    let miner_reward_state = reward_state.clone();
    let miner_to_miner_sender = to_miner_sender.clone();
    let miner_to_blockchain_sender = to_blockchain_sender.clone();
    let miner_to_gossip_sender = to_gossip_tx.clone();
    let miner_to_app_sender = to_app_sender.clone();
    let miner_node_id = node_id.clone();
    std::thread::spawn(move || {
        let mut miner = Miner::start(
            mining_wallet.clone().get_secretkey(),
            mining_wallet.clone().get_pubkey(),
            mining_wallet.clone().get_address(1),
            miner_reward_state,
            miner_network_state,
            0,
        );
        if let Err(_) = miner_to_app_sender
            .clone()
            .send(Command::UpdateAppMiner(miner.as_bytes()))
        {
            info!("Error sending miner to app");
        }
        loop {
            let blockchain_sender = miner_to_blockchain_sender.clone();
            let gossip_sender = miner_to_gossip_sender.clone();
            let miner_sender = miner_to_miner_sender.clone();
            let app_sender = miner_to_app_sender.clone();
            if let Ok(command) = to_miner_receiver.try_recv() {
                match command {
                    Command::SendMessage(src, message) => {

                        // TODO: Replace the below with sending to the correct channel
                        if let Err(e) = gossip_sender.send((src, message)) {
                            info!("Error sending to swarm receiver: {:?}", e);
                        }
                    }
                    Command::StartMiner => {
                        miner.mining = true;
                        if let Err(_) = miner_sender.send(Command::MineBlock) {
                            info!("Error sending mine block command to miner");
                        }
                    }
                    Command::MineBlock => {
                        if miner.mining {
                            if let Some(last_block) = miner.last_block.clone() {
                                if let Some(claim) =
                                    miner.clone().claim_map.get(&miner.clone().claim.pubkey)
                                {
                                    let lowest_pointer = miner.get_lowest_pointer(
                                        last_block.header.next_block_nonce as u128,
                                    );
                                    if let Some((hash, _)) = lowest_pointer.clone() {
                                        if hash == claim.hash.clone() {
                                            let block = miner.mine();
                                            if let Some(block) = block {
                                                let message = MessageType::BlockMessage {
                                                    block: block.clone().as_bytes(),
                                                    sender_id: miner_node_id.clone().to_string(),
                                                };

                                                let msg_id = MessageKey::rand();
                                                let gossip_msg = GossipMessage {
                                                    id: msg_id.inner(),
                                                    data: message.as_bytes(),
                                                    sender: addr.clone(),
                                                };

                                                let head = Header::Gossip;
                                                
                                                let msg = Message {
                                                    head,
                                                    msg: gossip_msg.as_bytes().unwrap()
                                                };

                                                miner.mining = false;

                                                // TODO: Replace the below with sending to the correct channel
                                                if let Err(e) = gossip_sender
                                                    .send((addr.clone(), msg))
                                                {
                                                    info!("Error sending SendMessage command to swarm: {:?}", e);
                                                }

                                                if let Err(_) =
                                                    blockchain_sender.send(Command::PendingBlock(
                                                        block.clone().as_bytes(),
                                                        miner_node_id.clone().to_string(),
                                                    ))
                                                {
                                                    info!("Error sending PendingBlock command to blockchain");
                                                }
                                            } else {
                                                if let Err(e) =
                                                    miner_sender.send(Command::MineBlock)
                                                {
                                                    info!(
                                                        "Error sending miner sender MineBlock: {:?}",
                                                        e
                                                    );
                                                }
                                            }
                                        } else {
                                            miner.mining = false;
                                            if let Err(_) =
                                                miner_sender.send(Command::CheckAbandoned)
                                            {
                                                info!("Error sending check abandoned command to miner");
                                            }
                                        }
                                    } else {
                                        if let Err(e) = miner_sender.send(Command::NonceUp) {
                                            info!(
                                                "Error sending NonceUp command to miner: {:?}",
                                                e
                                            );
                                        }
                                    }
                                }
                            } else {
                                if let Err(e) = miner_sender.send(Command::MineGenesis) {
                                    info!("Error sending mine genesis command to miner: {:?}", e);
                                };
                            }
                        }
                    }
                    Command::ConfirmedBlock(block_bytes) => {
                        let block = block::Block::from_bytes(&block_bytes);
                        miner.current_nonce_timer = block.header.timestamp;

                        if let Category::Motherlode(_) = block.header.block_reward.category {
                            info!("*****{:?}*****\n", &block.header.block_reward.category);
                        }
                        miner.last_block = Some(block.clone());
                        block.txns.iter().for_each(|(k, _)| {
                            miner.txn_pool.confirmed.remove(&k.clone());
                        });
                        let mut new_claims = block.claims.clone();
                        new_claims = new_claims
                            .iter()
                            .map(|(k, v)| {
                                return (k.clone(), v.clone());
                            })
                            .collect();
                        new_claims.iter().for_each(|(k, v)| {
                            miner.claim_pool.confirmed.remove(k);
                            miner.claim_map.insert(k.clone(), v.clone());
                        });

                        // Check if the miner's claim nonce changed,
                        // if it did change, make sure that it HAD to change.
                        // If it did have to change (nonce up) and your local claim map is different
                        // nonce up the local claim map until it is in consensus.
                        miner.claim_map.replace(
                            block.header.claim.clone().pubkey,
                            block.header.claim.clone(),
                        );

                        if let Err(_) = app_sender.send(Command::UpdateAppMiner(miner.as_bytes())) {
                            info!("Error sending updated miner to app")
                        }
                    }
                    Command::ProcessTxn(txn_bytes) => {
                        let txn = Txn::from_bytes(&txn_bytes);
                        let txn_validator = miner.process_txn(txn.clone());
                        miner.check_confirmed(txn.txn_id.clone());
                        let message = MessageType::TxnValidatorMessage {
                            txn_validator: txn_validator.as_bytes(),
                            sender_id: miner_node_id.clone(),
                        };

                        let head = Header::Gossip;
                        let msg_id = MessageKey::rand();
                        let gossip_msg = GossipMessage {
                            id: msg_id.inner(),
                            data: message.as_bytes(),
                            sender: addr.clone()
                        };

                        let msg = Message {
                            head,
                            msg: gossip_msg.as_bytes().unwrap()
                        };
                        

                        // TODO: Replace the below with sending to the correct channel
                        if let Err(e) = gossip_sender.send((addr.clone(), msg)) {
                            info!("Error sending SendMessage command to swarm: {:?}", e);
                        }
                        if let Err(_) = app_sender.send(Command::UpdateAppMiner(miner.as_bytes())) {
                            info!("Error sending updated miner to app.")
                        }
                    }
                    Command::ProcessClaim(claim_bytes) => {
                        let claim = Claim::from_bytes(&claim_bytes);
                        miner
                            .claim_pool
                            .confirmed
                            .insert(claim.pubkey.clone(), claim.clone());
                        if let Err(_) = app_sender.send(Command::UpdateAppMiner(miner.as_bytes())) {
                            info!("Error sending updated miner to app")
                        }
                    }
                    Command::ProcessTxnValidator(validator_bytes) => {
                        let validator = TxnValidator::from_bytes(&validator_bytes);
                        miner.process_txn_validator(validator.clone());
                        if let Some(bad_validators) =
                            miner.check_rejected(validator.txn.txn_id.clone())
                        {
                            if let Err(e) =
                                blockchain_sender.send(Command::SlashClaims(bad_validators.clone()))
                            {
                                info!(
                                    "Error sending SlashClaims command to blockchain thread: {:?}",
                                    e
                                );
                            }

                            bad_validators.iter().for_each(|k| {
                                miner.slash_claim(k.to_string());
                            });
                        } else {
                            miner.check_confirmed(validator.txn.txn_id.clone());
                        }

                        if let Err(_) = app_sender.send(Command::UpdateAppMiner(miner.as_bytes())) {
                            info!("Error sending updated miner to app")
                        }
                    }
                    Command::InvalidBlock(_) => {
                        if let Err(e) = miner_sender.send(Command::MineBlock) {
                            info!("Error sending mine block command to miner: {:?}", e);
                        }
                    }
                    Command::StateUpdateCompleted(network_state_bytes) => {
                        if let Ok(updated_network_state) = NetworkState::from_bytes(network_state_bytes) {
                            miner.network_state = updated_network_state.clone();
                            miner.claim_map = miner.network_state.get_claims();
                            miner.mining = true;
                            if let Err(e) = miner_sender.send(Command::MineBlock) {
                                info!("Error sending MineBlock command to miner: {:?}", e);
                            }
                            if let Err(_) = app_sender.send(Command::UpdateAppMiner(miner.as_bytes())) {
                                info!("Error sending updated miner to app")
                            }
                        }
                    }
                    Command::MineGenesis => {
                        if let Some(block) = miner.genesis() {
                            miner.mining = false;
                            miner.last_block = Some(block.clone());
                            let message = MessageType::BlockMessage {
                                block: block.clone().as_bytes(),
                                sender_id: miner_node_id.clone(),
                            };
                            let head = Header::Gossip;
                            let msg_id = MessageKey::rand();
                            let gossip_msg = GossipMessage {
                                id: msg_id.inner(),
                                data: message.as_bytes(),
                                sender: addr.clone()
                            };

                            let msg = Message {
                                head,
                                msg: gossip_msg.as_bytes().unwrap()
                            };
                            // TODO: Replace the below with sending to the correct channel
                            if let Err(e) = gossip_sender.send((addr.clone(), msg)) {
                                info!("Error sending SendMessage command to swarm: {:?}", e);
                            }
                            if let Err(_) = blockchain_sender.send(Command::PendingBlock(
                                block.clone().as_bytes(),
                                miner_node_id.clone(),
                            )) {
                                info!("Error sending to command receiver")
                            }
                            if let Err(_) =
                                app_sender.send(Command::UpdateAppMiner(miner.as_bytes()))
                            {
                                info!("Error sending updated miner to app")
                            }
                        }
                    }
                    Command::SendAddress => {
                        let message = MessageType::ClaimMessage {
                            claim: miner.claim.clone().as_bytes(),
                            sender_id: miner_node_id.clone(),
                        };
                        let head = Header::Gossip;
                        let msg_id = MessageKey::rand();
                        let gossip_msg = GossipMessage {
                            id: msg_id.inner(),
                            data: message.as_bytes(),
                            sender: addr.clone()
                        };

                        let msg = Message {
                            head,
                            msg: gossip_msg.as_bytes().unwrap()
                        };

                        // TODO: Replace the below with sending to the correct channel
                        if let Err(e) = gossip_sender.send((addr.clone(), msg)) {
                            info!("Error sending SendMessage command to swarm: {:?}", e);
                        }
                    }
                    Command::NonceUp => {
                        miner.nonce_up();
                        if let Err(e) = blockchain_sender.send(Command::NonceUp) {
                            info!("Error sending NonceUp command to blockchain: {:?}", e);
                        }
                        if let Err(_) = app_sender.send(Command::UpdateAppMiner(miner.as_bytes())) {
                            info!("Error sending updated miner to app")
                        }
                        if let Err(e) = miner_sender.send(Command::MineBlock) {
                            info!("Error sending MineBlock command to miner: {:?}", e);
                        }
                    }
                    Command::CheckAbandoned => {
                        if let Some(last_block) = miner.last_block.clone() {
                            if let Some(_) =
                                miner.clone().claim_map.get(&miner.clone().claim.pubkey)
                            {
                                let lowest_pointer = miner
                                    .get_lowest_pointer(last_block.header.next_block_nonce as u128);
                                if let Some((hash, _)) = lowest_pointer.clone() {
                                    if miner.check_time_elapsed() > 30 {
                                        miner.current_nonce_timer = miner.get_timestamp();
                                        let mut abandoned_claim_map = miner.claim_map.clone();
                                        abandoned_claim_map.retain(|_, v| v.hash == hash);

                                        if let Some((_, v)) = abandoned_claim_map.front() {
                                            let message = MessageType::ClaimAbandonedMessage {
                                                claim: v.clone().as_bytes(),
                                                sender_id: miner_node_id.clone(),
                                            };

                                            miner
                                                .abandoned_claim_counter
                                                .insert(miner.claim.pubkey.clone(), v.clone());

                                            let head = Header::Gossip;
                                            let msg_id = MessageKey::rand();
                                            let gossip_msg = GossipMessage {
                                                id: msg_id.inner(),
                                                data: message.as_bytes(),
                                                sender: addr.clone()
                                            };

                                            let msg = Message {
                                                head,
                                                msg: gossip_msg.as_bytes().unwrap()
                                            };
                                            // TODO: Replace the below with sending to the correct channel                                                
                                            if let Err(e) =
                                                gossip_sender.send((addr.clone(), msg))
                                            {
                                                info!("Error sending ClaimAbandoned message to swarm: {:?}", e);
                                            }

                                            let mut abandoned_claim_map =
                                                miner.abandoned_claim_counter.clone();

                                            abandoned_claim_map
                                                .retain(|_, claim| v.hash == claim.hash);

                                            if abandoned_claim_map.len() as f64
                                                / (miner.claim_map.len() as f64 - 1.0)
                                                > VALIDATOR_THRESHOLD
                                            {
                                                miner.claim_map.retain(|_, v| v.hash != hash);
                                                if let Err(e) =
                                                    blockchain_sender.send(Command::ClaimAbandoned(
                                                        miner.claim.pubkey.clone(),
                                                        v.clone().as_bytes(),
                                                    ))
                                                {
                                                    info!("Error forwarding confirmed abandoned claim to blockchain: {:?}", e);
                                                }
                                            }
                                        }
                                    } else {
                                        if let Err(_) = miner_sender.send(Command::CheckAbandoned) {
                                            info!("Error sending check abandoned command to miner");
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Command::ClaimAbandoned(pubkey, _) => {
                        if let Some(claim) = miner.claim_map.clone().get(&pubkey) {
                            miner
                                .abandoned_claim_counter
                                .insert(pubkey.clone(), claim.clone());

                            let mut abandoned_claim_map = miner.abandoned_claim_counter.clone();
                            abandoned_claim_map.retain(|_, v| v.hash == claim.hash);

                            if abandoned_claim_map.len() as f64
                                / (miner.claim_map.len() as f64 - 1.0)
                                > VALIDATOR_THRESHOLD
                            {
                                miner.claim_map.retain(|_, v| v.hash != claim.hash);
                            }
                            if let Err(_) =
                                app_sender.send(Command::UpdateAppMiner(miner.as_bytes()))
                            {
                                info!("Error sending updated miner to app")
                            }
                            miner.mining = true;
                            if let Err(e) = miner_sender.send(Command::MineBlock) {
                                info!("Error sending miner sender MineBlock: {:?}", e);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    });
    //____________________________________________________________________________________________________
    // State Sending Thread
    //____________________________________________________________________________________________________
    let state_to_swarm_sender = to_swarm_sender.clone();
    let state_to_gossip_sender = to_gossip_tx.clone();
    let state_to_blockchain_sender = to_blockchain_sender.clone();
    let state_node_id = node_id.clone();
    std::thread::spawn(move || loop {
        let blockchain_sender = state_to_blockchain_sender.clone();
        let swarm_sender = state_to_swarm_sender.clone();
        let gossip_sender = state_to_gossip_sender.clone();
        if let Ok(command) = to_state_receiver.try_recv() {
            match command {
                Command::SendStateComponents(requestor, component_bytes, sender_id) => {
                    let command = Command::GetStateComponents(requestor, component_bytes, sender_id);
                    if let Err(e) = blockchain_sender.send(command) {
                        info!("Error sending component request to blockchain thread: {:?}", e);
                    }
                }
                Command::StoreStateComponents(data, component_type) => {
                    if let Err(e) = blockchain_sender.send(Command::StoreStateComponents(data, component_type)) {
                        info!("Error sending component to blockchain")
                    }
                }
                Command::RequestedComponents(requestor, components, sender_id, requestor_id) => {
                    let restructured_components = Components::from_bytes(&components);
                    let head = Header::Gossip;
                    let genesis_message = MessageType::GenesisMessage {
                        data: restructured_components.genesis.unwrap(),
                        requestor: requestor.clone(),
                        requestor_id: requestor_id.clone(),
                        sender_id: state_node_id.clone(),
                    };

                    let child_message = MessageType::ChildMessage {
                        data: restructured_components.child.unwrap(),
                        requestor: requestor.clone(),
                        requestor_id: requestor_id.clone(),
                        sender_id: state_node_id.clone(),
                    };

                    let parent_message = MessageType::ParentMessage {
                        data: restructured_components.parent.unwrap(),
                        requestor: requestor.clone(),
                        requestor_id: requestor_id.clone(),
                        sender_id: state_node_id.clone(),
                    };

                    let ledger_message = MessageType::LedgerMessage {
                        data: restructured_components.ledger.unwrap(),
                        requestor: requestor.clone(),
                        requestor_id: requestor_id.clone(),
                        sender_id: state_node_id.clone(),
                    };

                    let network_state_message = MessageType::NetworkStateMessage {
                        data: restructured_components.network_state.unwrap(),
                        requestor: requestor.clone(),
                        requestor_id: requestor_id.clone(),
                        sender_id: state_node_id.clone(),
                    };

                    
                    let genesis_msg_id = MessageKey::rand();
                    let child_msg_id = MessageKey::rand();
                    let parent_msg_id = MessageKey::rand();
                    let ledger_msg_id = MessageKey::rand();
                    let network_state_msg_id = MessageKey::rand();
                    
                    let genesis_gossip_message = GossipMessage {
                        id: genesis_msg_id.inner(),
                        data: genesis_message.as_bytes(),
                        sender: addr.clone(),
                    };

                    let child_gossip_message = GossipMessage {
                        id: child_msg_id.inner(),
                        data: child_message.as_bytes(),
                        sender: addr.clone(),
                    };

                    let parent_gossip_message = GossipMessage {
                        id: parent_msg_id.inner(),
                        data: parent_message.as_bytes(),
                        sender: addr.clone(),
                    };

                    let ledger_gossip_message = GossipMessage {
                        id: ledger_msg_id.inner(),
                        data: ledger_message.as_bytes(),
                        sender: addr.clone(),
                    };
                    
                    let network_state_gossip_message = GossipMessage {
                        id: network_state_msg_id.inner(),
                        data: network_state_message.as_bytes(),
                        sender: addr.clone(),
                    };

                    
                    let genesis_msg = Message {
                        head: head.clone(),
                        msg: genesis_gossip_message.as_bytes().unwrap()
                    };

                    let child_msg = Message {
                        head: head.clone(),
                        msg: child_gossip_message.as_bytes().unwrap()
                    };

                    let parent_msg = Message {
                        head: head.clone(),
                        msg: parent_gossip_message.as_bytes().unwrap()
                    };
                    
                    let ledger_msg = Message {
                        head: head.clone(),
                        msg: ledger_gossip_message.as_bytes().unwrap()
                    };
                    
                    let network_state_msg = Message {
                        head: head.clone(),
                        msg: network_state_gossip_message.as_bytes().unwrap()
                    };                    
                    
                    let messages: Vec<Message> = vec![genesis_msg, child_msg, parent_msg, ledger_msg, network_state_msg];

                    let requestor_addr: SocketAddr = requestor.parse().expect("Unable to parse address");
                    
                    match requestor_addr {
                        SocketAddr::V4(v4) => {
                            let ip = v4.ip().clone();
                            let port = 19291;
                            let new_addr = SocketAddrV4::new(ip, port);
                            let tcp_addr = SocketAddr::from(new_addr);
                            match std::net::TcpStream::connect(new_addr) {
                                Ok(mut stream) => {

                                    for message in messages {
                                        let msg_bytes = message.as_bytes().unwrap();
                                        stream.write(&msg_bytes).unwrap();
                                    }
                                }
                                Err(_) => {}
                            }
                        }
                        SocketAddr::V6(v6) => {}
                    }
                    
                    
                }
                _ => {
                    info!("Received State Command: {:?}", command);
                }
            }
        }
    });

    let terminal_to_swarm_sender = to_swarm_sender.clone();
    let terminal_to_gossip_sender = to_gossip_tx.clone();
    let terminal_node_id = node_id.clone();
    loop {
        let swarm_sender = terminal_to_swarm_sender.clone();
        let gossip_sender = terminal_to_gossip_sender.clone();
        terminal.draw(|rect| {
            if let Ok(command) = to_app_receiver.try_recv() {
                match command {
                    Command::UpdateAppBlockchain(blockchain_bytes) => {
                        app.blockchain = Some(Blockchain::from_bytes(&blockchain_bytes));
                    }
                    Command::UpdateAppMiner(miner_bytes) => {
                        app.miner = Some(Miner::from_bytes(&miner_bytes));
                    }
                    Command::UpdateAppWallet(wallet_bytes) => {
                        app.wallet = Some(WalletAccount::from_bytes(&wallet_bytes));
                    }
                    Command::UpdateAppMessageCache(message_bytes) => {
                        let next = app.message_cache.len().clone() as u128;
                        if let Some(message) = MessageType::from_bytes(&message_bytes) {
                            app.message_cache.insert(next, message);
                        }
                    }
                    _ => {}
                }
            }
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .constraints(
                    [
                        Constraint::Length(3),
                        Constraint::Length(3),
                        Constraint::Length(3),
                        Constraint::Min(2),
                        Constraint::Length(3),
                    ]
                    .as_ref(),
                )
                .split(rect.size());
            let copyright = Paragraph::new("VRRB-CLI 2021 - all rights reserved")
                .style(Style::default().fg(Color::LightCyan))
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .style(Style::default().fg(Color::White))
                        .title("Copyright")
                        .border_type(BorderType::Plain),
                );
            let menu = menu_titles
                .iter()
                .map(|t| {
                    let (first, rest) = t.split_at(1);
                    Spans::from(vec![
                        Span::styled(
                            first,
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::UNDERLINED),
                        ),
                        Span::styled(rest, Style::default().fg(Color::White)),
                    ])
                })
                .collect();
            let tabs = Tabs::new(menu)
                .select(active_menu_item.into())
                .block(Block::default().title("Menu").borders(Borders::ALL))
                .style(Style::default().fg(Color::White))
                .highlight_style(Style::default().fg(Color::Yellow))
                .divider(Span::raw("|"));

            rect.render_widget(tabs, chunks[2]);

            match active_menu_item {
                MenuItem::Home => rect.render_widget(render_home(&addr.to_string(), &wallet), chunks[3]),
                MenuItem::Wallet => {
                    let wallet_chunks = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints(
                            [Constraint::Percentage(20), Constraint::Percentage(80)].as_ref(),
                        )
                        .split(chunks[3]);

                    let app_network_state = {
                        if let Some(miner) = app.clone().miner {
                            miner.network_state.clone()
                        } else {
                            network_state.clone()
                        }
                    };

                    let (left, right) = render_wallet(
                        &wallet_list_state,
                        wallet.get_wallet_addresses(),
                        get_credits(&app_network_state.db_to_ledger()),
                        get_debits(&app_network_state.db_to_ledger()),
                    );
                    rect.render_stateful_widget(left, wallet_chunks[0], &mut wallet_list_state);
                    rect.render_widget(right, wallet_chunks[1]);
                }
                MenuItem::Mining
                | MenuItem::ClaimMap
                | MenuItem::TxnPool
                | MenuItem::ClaimPool
                | MenuItem::PendingClaims
                | MenuItem::ConfirmedClaims
                | MenuItem::PendingTxns
                | MenuItem::ConfirmedTxns => {
                    let mining_data_chunks = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints(
                            [Constraint::Percentage(20), Constraint::Percentage(80)].as_ref(),
                        )
                        .split(chunks[3]);
                    if let Some(miner) = app.clone().miner {
                        let field_titles = miner.get_field_names();
                        let left = render_miner_list(&miner_fields, field_titles.clone());
                        rect.render_stateful_widget(left, mining_data_chunks[0], &mut miner_fields);

                        if let Some(selected) = miner_fields.selected() {
                            if let Some(field) = field_titles.clone().get(selected) {
                                match &field[..] {
                                    "claim" => {
                                        let data = render_claim_data(&miner.claim.clone());
                                        rect.render_widget(data, mining_data_chunks[1]);
                                    }
                                    "mining" => {}
                                    "claim_map" => {
                                        let claim_map_data_chunks = Layout::default()
                                            .direction(Direction::Horizontal)
                                            .constraints(
                                                [
                                                    Constraint::Percentage(15),
                                                    Constraint::Percentage(85),
                                                ]
                                                .as_ref(),
                                            )
                                            .split(mining_data_chunks[1]);
                                        let (pubkeys, data) = render_claim_map(
                                            &claim_map_list_state,
                                            &miner.claim_map.clone(),
                                        );

                                        rect.render_stateful_widget(
                                            pubkeys,
                                            claim_map_data_chunks[0],
                                            &mut claim_map_list_state,
                                        );
                                        rect.render_widget(data, claim_map_data_chunks[1]);
                                    }
                                    "txn_pool" => {
                                        let txn_pool_data_chunks = Layout::default()
                                            .direction(Direction::Horizontal)
                                            .constraints(
                                                [
                                                    Constraint::Percentage(30),
                                                    Constraint::Percentage(70),
                                                ]
                                                .as_ref(),
                                            )
                                            .split(mining_data_chunks[1]);
                                        let txn_and_status_data_chunks = Layout::default()
                                            .direction(Direction::Horizontal)
                                            .constraints(
                                                [
                                                    Constraint::Percentage(50),
                                                    Constraint::Percentage(50),
                                                ]
                                                .as_ref(),
                                            )
                                            .split(txn_pool_data_chunks[0]);

                                        let (status, txn_id, data) = render_txn_pool(
                                            &txn_pool_status_list_state,
                                            &txn_pool_list_state,
                                            &miner.txn_pool.clone(),
                                        );

                                        rect.render_stateful_widget(
                                            status,
                                            txn_and_status_data_chunks[0],
                                            &mut txn_pool_status_list_state,
                                        );
                                        rect.render_stateful_widget(
                                            txn_id,
                                            txn_and_status_data_chunks[1],
                                            &mut txn_pool_list_state,
                                        );
                                        rect.render_widget(data, txn_pool_data_chunks[1]);
                                    }
                                    "claim_pool" => {
                                        let claim_pool_data_chunks = Layout::default()
                                            .direction(Direction::Horizontal)
                                            .constraints(
                                                [
                                                    Constraint::Percentage(30),
                                                    Constraint::Percentage(70),
                                                ]
                                                .as_ref(),
                                            )
                                            .split(mining_data_chunks[1]);
                                        let claim_and_status_data_chunks = Layout::default()
                                            .direction(Direction::Horizontal)
                                            .constraints(
                                                [
                                                    Constraint::Percentage(50),
                                                    Constraint::Percentage(50),
                                                ]
                                                .as_ref(),
                                            )
                                            .split(claim_pool_data_chunks[0]);

                                        let (status, pubkey, data) = render_claim_pool(
                                            &claim_pool_status_list_state,
                                            &claim_pool_list_state,
                                            &miner.claim_pool.clone(),
                                        );

                                        rect.render_stateful_widget(
                                            status,
                                            claim_and_status_data_chunks[0],
                                            &mut claim_pool_status_list_state,
                                        );
                                        rect.render_stateful_widget(
                                            pubkey,
                                            claim_and_status_data_chunks[1],
                                            &mut claim_pool_list_state,
                                        );
                                        rect.render_widget(data, claim_pool_data_chunks[1]);
                                    }
                                    "last_block" => {
                                        let table = {
                                            if let Some(block) = miner.last_block.clone() {
                                                render_block_table(&block)
                                            } else {
                                                render_empty_table()
                                            }
                                        };

                                        rect.render_widget(table, mining_data_chunks[1]);
                                    }
                                    "reward_state" => {
                                        if let Some(reward_state) = miner.network_state.reward_state.clone() {
                                            rect.render_widget(
                                                render_reward_state(
                                                    &reward_state.clone(),
                                                ),
                                                mining_data_chunks[1],
                                            );
                                        }
                                    }
                                    "network_state" => rect.render_widget(
                                        render_network_state(&miner.network_state.clone()),
                                        mining_data_chunks[1],
                                    ),
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                MenuItem::Network => {
                    rect.render_widget(render_network_data(&events_path.clone()), chunks[3]);
                }
                MenuItem::ChainData | MenuItem::HeaderChain | MenuItem::InvalidBlocks => {
                    let chain_data_chunks = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints(
                            [Constraint::Percentage(20), Constraint::Percentage(80)].as_ref(),
                        )
                        .split(chunks[3]);

                    if let Some(blockchain) = app.clone().blockchain {
                        let field_titles = blockchain.get_field_names();
                        let left = render_chain_list(&blockchain_fields, field_titles.clone());
                        rect.render_stateful_widget(
                            left,
                            chain_data_chunks[0],
                            &mut blockchain_fields,
                        );
                        if let Some(selected) = blockchain_fields.selected() {
                            if let Some(field) = field_titles.clone().get(selected) {
                                match &field[..] {
                                    "genesis" => {
                                        if let Some(genesis) = blockchain.genesis.clone() {
                                            let first = render_block_table(&genesis);
                                            rect.render_widget(first, chain_data_chunks[1]);
                                        }
                                    }
                                    "child" => {
                                        if let Some(child) = blockchain.child.clone() {
                                            let first = render_block_table(&child);
                                            rect.render_widget(first, chain_data_chunks[1]);
                                        }
                                    }
                                    "parent" => {
                                        if let Some(parent) = blockchain.parent.clone() {
                                            let first = render_block_table(&parent);
                                            rect.render_widget(first, chain_data_chunks[1]);
                                        }
                                    }
                                    "chain" => {
                                        let chain_headers_data_chunks = Layout::default()
                                            .direction(Direction::Horizontal)
                                            .constraints(
                                                [
                                                    Constraint::Percentage(20),
                                                    Constraint::Percentage(80),
                                                ]
                                                .as_ref(),
                                            )
                                            .split(chain_data_chunks[1]);

                                        let (chain, header) = render_header_chain(
                                            &last_hash_list_state,
                                            &blockchain.chain.clone(),
                                        );
                                        rect.render_stateful_widget(
                                            chain,
                                            chain_headers_data_chunks[0],
                                            &mut last_hash_list_state,
                                        );
                                        rect.render_widget(header, chain_headers_data_chunks[1]);
                                    }
                                    "invalid" => {
                                        let invalid_block_data_chunks = Layout::default()
                                            .direction(Direction::Horizontal)
                                            .constraints(
                                                [
                                                    Constraint::Percentage(20),
                                                    Constraint::Percentage(80),
                                                ]
                                                .as_ref(),
                                            )
                                            .split(chain_data_chunks[1]);

                                        let (hash, block) = render_invalid_blocks(
                                            &invalid_block_list_state,
                                            &blockchain.invalid.clone(),
                                        );

                                        rect.render_stateful_widget(
                                            hash,
                                            invalid_block_data_chunks[0],
                                            &mut invalid_block_list_state,
                                        );
                                        rect.render_widget(block, invalid_block_data_chunks[1]);
                                    }
                                    "chain_db" => {
                                        rect.render_widget(
                                            render_chain_db(&blockchain.chain_db),
                                            chain_data_chunks[1],
                                        );
                                    }
                                    _ => {}
                                }
                            }
                        }
                        // rect.render_widget(right, chain_data_chunks[1]);
                    }
                }
                MenuItem::Commands => {
                    rect.render_widget(
                        render_command_cache(&command_input.command_cache),
                        chunks[3],
                    );
                }
            }
            let (msg, style) = match command_input.input_mode {
                InputMode::Normal => (
                    vec![
                        Span::raw("Press "),
                        Span::styled("q", Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(" to exit, "),
                        Span::styled("c", Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(" to enter a command "),
                    ],
                    Style::default().add_modifier(Modifier::RAPID_BLINK),
                ),
                InputMode::Editing => (
                    vec![
                        Span::raw("Press "),
                        Span::styled("Esc ", Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw("to exit command entry mode "),
                        Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(" to execute the command"),
                    ],
                    Style::default(),
                ),
            };

            let mut text = Text::from(Spans::from(msg));
            text.patch_style(style);
            let help_message = Paragraph::new(text);
            rect.render_widget(help_message, chunks[0]);
            let input = Paragraph::new(command_input.input.as_ref())
                .style(match command_input.input_mode {
                    InputMode::Normal => Style::default(),
                    InputMode::Editing => Style::default().fg(Color::Yellow),
                })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Command Input"),
                );
            rect.render_widget(input, chunks[1]);
            match command_input.input_mode {
                InputMode::Normal => {}
                InputMode::Editing => rect.set_cursor(
                    chunks[1].x + command_input.input.width() as u16 + 1,
                    chunks[1].y + 1,
                ),
            }
            rect.render_widget(copyright, chunks[4]);
        })?;

        if let Ok(event) = rx.try_recv() {
            match command_input.input_mode {
                InputMode::Normal => match event {
                    Event::Tick => {}
                    Event::Input(event) => match event.code {
                        KeyCode::Char('q') => {
                            disable_raw_mode()?;
                            terminal.clear()?;
                            terminal.show_cursor()?;
                            break;
                        }
                        KeyCode::Char('h') => active_menu_item = MenuItem::Home,
                        KeyCode::Char('w') => active_menu_item = MenuItem::Wallet,
                        KeyCode::Char('m') => active_menu_item = MenuItem::Mining,
                        KeyCode::Char('n') => active_menu_item = MenuItem::Network,
                        KeyCode::Char('d') => active_menu_item = MenuItem::ChainData,
                        KeyCode::Char('c') => {
                            command_input.input_mode = InputMode::Editing;
                        }
                        KeyCode::Char('e') => {
                            if let MenuItem::Commands = active_menu_item {
                                command_input.input_mode = InputMode::Editing;
                            }
                        }
                        KeyCode::Down => {
                            if let MenuItem::Wallet = active_menu_item {
                                if let Some(selected) = wallet_list_state.selected() {
                                    let n_addresses = wallet.get_wallet_addresses().len();
                                    if selected >= n_addresses - 1 {
                                        wallet_list_state.select(Some(0));
                                    } else {
                                        wallet_list_state.select(Some(selected + 1))
                                    }
                                }
                            }

                            if let MenuItem::ChainData = active_menu_item {
                                if let Some(selected) = blockchain_fields.selected() {
                                    if let Some(blockchain) = app.clone().blockchain {
                                        let n_fields = blockchain.get_field_names().len();
                                        if selected >= n_fields - 1 {
                                            blockchain_fields.select(Some(0));
                                        } else {
                                            blockchain_fields.select(Some(selected + 1));
                                        }
                                    }
                                }
                            }

                            if let MenuItem::HeaderChain = active_menu_item {
                                if let Some(blockchain) = app.clone().blockchain {
                                    if let Some(selected) = last_hash_list_state.selected() {
                                        let n_last_hashes = blockchain.clone().chain.len();
                                        if selected >= n_last_hashes - 1 {
                                            last_hash_list_state.select(Some(0));
                                        } else {
                                            last_hash_list_state.select(Some(selected + 1));
                                        }
                                    }
                                }
                            }

                            if let MenuItem::InvalidBlocks = active_menu_item {
                                if let Some(blockchain) = app.clone().blockchain {
                                    if let Some(selected) = invalid_block_list_state.selected() {
                                        let n_invalid_blocks = blockchain.clone().invalid.len();
                                        if n_invalid_blocks > 0 {
                                            if selected >= n_invalid_blocks - 1 {
                                                invalid_block_list_state.select(Some(0));
                                            } else {
                                                invalid_block_list_state.select(Some(selected + 1));
                                            }
                                        }
                                    }
                                }
                            }

                            if let MenuItem::Mining = active_menu_item {
                                if let Some(miner) = app.clone().miner {
                                    if let Some(selected) = miner_fields.selected() {
                                        let n_fields = miner.get_field_names().len();
                                        if selected >= n_fields - 1 {
                                            miner_fields.select(Some(0));
                                        } else {
                                            miner_fields.select(Some(selected + 1));
                                        }
                                    }
                                }
                            }

                            if let MenuItem::ClaimMap = active_menu_item {
                                if let Some(miner) = app.clone().miner {
                                    if let Some(selected) = claim_map_list_state.selected() {
                                        let n_claims = miner.claim_map.clone().len();
                                        if n_claims > 0 {
                                            if selected >= n_claims - 1 {
                                                claim_map_list_state.select(Some(0));
                                            } else {
                                                claim_map_list_state.select(Some(selected + 1));
                                            }
                                        }
                                    }
                                }
                            }

                            if let MenuItem::ClaimPool = active_menu_item {
                                if let Some(_) = app.clone().miner {
                                    if let Some(selected) = claim_pool_status_list_state.selected()
                                    {
                                        let n_fields = 2;
                                        if selected >= n_fields - 1 {
                                            claim_pool_status_list_state.select(Some(0));
                                        } else {
                                            claim_pool_status_list_state.select(Some(selected + 1));
                                        }
                                    }
                                }
                            }

                            if let MenuItem::TxnPool = active_menu_item {
                                if let Some(_) = app.clone().miner {
                                    if let Some(selected) = txn_pool_status_list_state.selected() {
                                        let n_fields = 2;
                                        if selected >= n_fields - 1 {
                                            txn_pool_status_list_state.select(Some(0));
                                        } else {
                                            txn_pool_status_list_state.select(Some(selected + 1));
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Up => {
                            if let MenuItem::Wallet = active_menu_item {
                                if let Some(selected) = wallet_list_state.selected() {
                                    let n_addresses = wallet.get_wallet_addresses().len();
                                    if selected > 0 {
                                        wallet_list_state.select(Some(selected - 1));
                                    } else {
                                        wallet_list_state.select(Some(n_addresses - 1))
                                    }
                                }
                            }

                            if let MenuItem::ChainData = active_menu_item {
                                if let Some(selected) = blockchain_fields.selected() {
                                    if let Some(blockchain) = app.clone().blockchain {
                                        let n_fields = blockchain.get_field_names().len();
                                        if selected > 0 {
                                            blockchain_fields.select(Some(selected - 1));
                                        } else {
                                            blockchain_fields.select(Some(n_fields - 1));
                                        }
                                    }
                                }
                            }

                            if let MenuItem::HeaderChain = active_menu_item {
                                if let Some(selected) = last_hash_list_state.selected() {
                                    if let Some(blockchain) = app.clone().blockchain {
                                        let n_last_hashes = blockchain.clone().chain.len();
                                        if n_last_hashes > 0 {
                                            if selected > 0 {
                                                last_hash_list_state.select(Some(selected - 1));
                                            } else {
                                                last_hash_list_state
                                                    .select(Some(n_last_hashes - 1));
                                            }
                                        }
                                    }
                                }
                            }

                            if let MenuItem::InvalidBlocks = active_menu_item {
                                if let Some(selected) = invalid_block_list_state.selected() {
                                    if let Some(blockchain) = app.clone().blockchain {
                                        let n_invalid_blocks = blockchain.clone().invalid.len();
                                        if n_invalid_blocks > 0 {
                                            if selected > 0 {
                                                invalid_block_list_state.select(Some(selected - 1));
                                            } else {
                                                invalid_block_list_state
                                                    .select(Some(n_invalid_blocks - 1));
                                            }
                                        }
                                    }
                                }
                            }

                            if let MenuItem::Mining = active_menu_item {
                                if let Some(selected) = miner_fields.selected() {
                                    if let Some(miner) = app.clone().miner {
                                        let n_fields = miner.get_field_names().len();
                                        if selected > 0 {
                                            miner_fields.select(Some(selected - 1));
                                        } else {
                                            miner_fields.select(Some(n_fields - 1));
                                        }
                                    }
                                }
                            }

                            if let MenuItem::ClaimMap = active_menu_item {
                                if let Some(selected) = claim_map_list_state.selected() {
                                    if let Some(miner) = app.clone().miner {
                                        let n_claims = miner.claim_map.clone().len();
                                        if n_claims > 0 {
                                            if selected > 0 {
                                                claim_map_list_state.select(Some(selected - 1));
                                            } else {
                                                claim_map_list_state.select(Some(n_claims - 1));
                                            }
                                        }
                                    }
                                }
                            }

                            if let MenuItem::ClaimPool = active_menu_item {
                                if let Some(selected) = claim_pool_status_list_state.selected() {
                                    if let Some(_) = app.clone().miner {
                                        let n_fields = 2;
                                        if selected > 0 {
                                            claim_pool_status_list_state.select(Some(selected - 1));
                                        } else {
                                            claim_pool_status_list_state.select(Some(n_fields - 1));
                                        }
                                    }
                                }
                            }

                            if let MenuItem::TxnPool = active_menu_item {
                                if let Some(selected) = txn_pool_status_list_state.selected() {
                                    if let Some(_) = app.clone().miner {
                                        let n_fields = 2;
                                        if selected > 0 {
                                            txn_pool_status_list_state.select(Some(selected - 1));
                                        } else {
                                            txn_pool_status_list_state.select(Some(n_fields - 1));
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Right => {
                            if let MenuItem::ChainData = active_menu_item {
                                if let Some(selected) = blockchain_fields.selected() {
                                    if let Some(blockchain) = app.clone().blockchain {
                                        let field_selected =
                                            &blockchain.get_field_names().clone()[selected];
                                        if field_selected == "chain" {
                                            active_menu_item = MenuItem::HeaderChain;
                                        } else if field_selected == "invalid" {
                                            active_menu_item = MenuItem::InvalidBlocks;
                                        }
                                    }
                                }
                            }

                            if let MenuItem::ClaimPool = active_menu_item {
                                if let Some(selected) = claim_pool_status_list_state.selected() {
                                    if let Some(_) = app.clone().miner {
                                        if selected == 0 {
                                            active_menu_item = MenuItem::PendingClaims;
                                        } else {
                                            active_menu_item = MenuItem::ConfirmedClaims;
                                        }
                                    }
                                }
                            }

                            if let MenuItem::TxnPool = active_menu_item {
                                if let Some(selected) = txn_pool_status_list_state.selected() {
                                    if let Some(_) = app.clone().miner {
                                        if selected == 0 {
                                            active_menu_item = MenuItem::PendingTxns;
                                        } else {
                                            active_menu_item = MenuItem::ConfirmedTxns;
                                        }
                                    }
                                }
                            }

                            if let MenuItem::Mining = active_menu_item {
                                if let Some(selected) = miner_fields.selected() {
                                    if let Some(miner) = app.clone().miner {
                                        let field_selected =
                                            &miner.get_field_names().clone()[selected];
                                        if field_selected == "claim_map" {
                                            active_menu_item = MenuItem::ClaimMap;
                                        } else if field_selected == "claim_pool" {
                                            active_menu_item = MenuItem::ClaimPool;
                                        } else if field_selected == "txn_pool" {
                                            active_menu_item = MenuItem::TxnPool;
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Left => {
                            if let MenuItem::HeaderChain | MenuItem::InvalidBlocks =
                                active_menu_item
                            {
                                active_menu_item = MenuItem::ChainData;
                            }

                            if let MenuItem::ClaimMap | MenuItem::ClaimPool | MenuItem::TxnPool =
                                active_menu_item
                            {
                                active_menu_item = MenuItem::Mining;
                            }

                            if let MenuItem::PendingClaims | MenuItem::ConfirmedClaims =
                                active_menu_item
                            {
                                active_menu_item = MenuItem::ClaimPool;
                            }

                            if let MenuItem::PendingTxns | MenuItem::ConfirmedTxns =
                                active_menu_item
                            {
                                active_menu_item = MenuItem::TxnPool;
                            }
                        }
                        _ => {}
                    },
                },
                InputMode::Editing => match event {
                    Event::Tick => {}
                    Event::Input(event) => match event.code {
                        KeyCode::Char(c) => {
                            if c == 'v' && event.modifiers == KeyModifiers::CONTROL {
                                let mut ctx: ClipboardContext = ClipboardProvider::new().unwrap();
                                if let Ok(string) = ctx.get_contents() {
                                    string.chars().for_each(|c| {
                                        command_input.input.push(c);
                                    });
                                }
                            } else {
                                command_input.input.push(c);
                            }
                        }
                        KeyCode::Backspace => {
                            command_input.input.pop();
                        }
                        KeyCode::Esc => {
                            command_input.input_mode = InputMode::Normal;
                            active_menu_item = MenuItem::Commands;
                        }
                        KeyCode::Enter => {
                            if let Some(command) = Command::from_str(&command_input.input) {
                                match command.clone() {
                                    Command::SendTxn(addr_num, receiver, amount) => {
                                        let txn =
                                            wallet.clone().send_txn(addr_num, receiver, amount);
                                        if let Ok(txn) = txn {
                                            let message = MessageType::TxnMessage {
                                                txn: txn.as_bytes(),
                                                sender_id: terminal_node_id.clone(),
                                            };
                                            let head = Header::Gossip;
                                            let msg_id = MessageKey::rand();
                                            let gossip_msg = GossipMessage {
                                                id: msg_id.inner(),
                                                data: message.as_bytes(),
                                                sender: addr.clone()
                                            };

                                            let msg = Message {
                                                head,
                                                msg: gossip_msg.as_bytes().unwrap()
                                            };
                                            if let Err(e) =
                                                gossip_sender.send((addr.clone(), msg))
                                            {
                                                info!("Error sending to command receiver: {:?}", e);
                                            };
                                        }
                                    }
                                    _ => {
                                        if let Err(_) = command_sender.send(command) {
                                            info!("Error sending command to command receiver");
                                        };
                                    }
                                }
                            }
                            command_input
                                .command_cache
                                .push(command_input.input.drain(..).collect());

                            active_menu_item = MenuItem::Commands;
                        }
                        _ => {}
                    },
                },
            }
        }
    }

    Ok(())
}
