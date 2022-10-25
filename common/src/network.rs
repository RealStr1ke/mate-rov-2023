use std::fmt::Debug;
use std::net::{ToSocketAddrs};
use std::time::{Duration, Instant};
use anyhow::{bail, Context};
use message_io::network::{Endpoint, NetEvent, SendStatus, ToRemoteAddr, Transport};
use message_io::node::{NodeEvent, NodeHandler, NodeTask};
use tracing::{trace, info, error};
use crate::protocol::Packet;

const TIMEOUT: Duration = Duration::from_secs(10);

pub struct Network {
    handler: NodeHandler<WorkerEvent>,
    task: NodeTask
}

struct NetworkContext<EventHandler> {
    handler: NodeHandler<WorkerEvent>,
    connection: Option<Connection>,
    events: EventHandler
}

pub trait EventHandler: Sized + Debug {
    fn handle_packet(&mut self, handler: &NodeHandler<WorkerEvent>, connection: &Connection, packet: Packet) -> anyhow::Result<()>;

    fn connected(&mut self, _endpoint: Endpoint) -> anyhow::Result<()> { Ok(()) }
    fn connection_failed(&mut self, _endpoint: Endpoint) -> anyhow::Result<()> { Ok(()) }
    fn disconnected(&mut self, _endpoint: Endpoint) -> anyhow::Result<()> { Ok(()) }
}

impl Network {
    #[tracing::instrument]
    pub fn create<Events: EventHandler + Send + 'static>(events: Events) -> Self {
        trace!("Create Network");

        let (handler, listener) = message_io::node::split::<WorkerEvent>();

        let task = {
            let mut ctx = NetworkContext {
                handler: handler.clone(),
                connection: None,
                events
            };

            listener.for_each_async(move |event| {
                handle_event(&mut ctx, event);
            })
        };

        Network {
            handler,
            task
        }
    }

    #[tracing::instrument(skip(self))]
    pub fn listen(&self, addrs: impl ToSocketAddrs + Debug) -> anyhow::Result<()> {
        trace!("Starting server on {:?}", addrs);

        self.handler.network().listen(Transport::FramedTcp, addrs).context("Bind to port")?;

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub fn connect(&self, addrs: impl ToRemoteAddr + Debug) -> anyhow::Result<()> {
        trace!("Connecting to server on {:?}", addrs);

        self.handler.network().connect(Transport::FramedTcp, addrs).context("Bind to port")?;

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub fn stop(&mut self) {
        self.handler.stop();
        self.task.wait();
    }

    #[tracing::instrument(skip(self))]
    pub fn send_packet(&self, packet: Packet) {
        self.handler.signals().send(WorkerEvent::Broadcast(packet));
    }
}

#[derive(Debug)]
pub struct Connection {
    endpoint: Endpoint,
    last_packet: Instant,
}

impl Connection {
    #[tracing::instrument(skip(handler))]
    pub fn write_packet(&self, handler: &NodeHandler<WorkerEvent>, packet: Packet) -> anyhow::Result<()> {
        trace!(?packet);

        let data: Vec<u8> = (&packet).try_into().context("Encode packet")?;

        let ret = handler.network().send(self.endpoint, &data);
        match ret {
            SendStatus::Sent => {}
            err => bail!("Could not send packet: {:?}", err)
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum WorkerEvent {
    Broadcast(Packet),
    // TODO
}

#[tracing::instrument(skip(network))]
fn handle_event<Events: EventHandler>(network: &mut NetworkContext<Events>, event: NodeEvent<WorkerEvent>) {
    trace!(?event);
    match event {
        NodeEvent::Network(event) => {
            let ret = handle_network_event(network, event);
            if let Err(err) = ret {
                error!("Error handling packet: {:?}", err)
            }
        }
        NodeEvent::Signal(event) => {
            let ret = handle_signal_event(network, event);
            if let Err(err) = ret {
                error!("Error handling signal: {:?}", err)
            }
        }
    }
}

#[tracing::instrument(skip(network))]
fn handle_network_event<Events: EventHandler>(network: &mut NetworkContext<Events>, event: NetEvent) -> anyhow::Result<()> {
    trace!(?event);
    match event {
        NetEvent::Accepted(endpoint, _resource_id) => {
            info!("Got connection from {}", endpoint);

            let new = Connection {
                endpoint,
                last_packet: Instant::now()
            };
            let previous = network.connection.take();

            if let Some(previous) = previous {
                if previous.last_packet.elapsed() > TIMEOUT {
                    network.connection = Some(new);
                    network.events.connected(endpoint).context("Connected event")?;
                } else {
                    network.connection = Some(previous);
                }
            } else {
                network.connection = Some(new);
                network.events.connected(endpoint).context("Connected event")?;
            }
        }
        NetEvent::Connected(endpoint, success) => {
            if success {
                info!("Connected to {}", endpoint);

                network.connection = Some(Connection {
                    endpoint,
                    last_packet: Instant::now()
                });

                network.events.connected(endpoint).context("Connected event")?;
            } else {
                error!("Could not connect to endpoint: {}", endpoint);
                network.events.connection_failed(endpoint).context("Connection failed event")?;
            }
        },
        NetEvent::Message(endpoint, data) => {
            trace!("Message from endpoint: {}", endpoint);
            let packet = data.try_into().context("Decode packet")?;

            if let Some(connection) = &mut network.connection {
                trace!(?packet);

                connection.last_packet = Instant::now();

                network.events.handle_packet(&network.handler, connection, packet).context("Handle packet event")?;
            } else {
                error!("Got packet from unknown endpoint");
            }
        }
        NetEvent::Disconnected(endpoint) => {
            info!("Endpoint {} disconnected", endpoint);
            network.connection = None;
            network.events.disconnected(endpoint).context("Disconnected event")?;
        }
    }

    Ok(())
}

#[tracing::instrument(skip(network))]
fn handle_signal_event<Events: EventHandler>(network: &mut NetworkContext<Events>, event: WorkerEvent) -> anyhow::Result<()> {
    trace!(?event);
    match event {
        WorkerEvent::Broadcast(packet) => {
            if let Some(ref connection) = network.connection {
                connection.write_packet(&network.handler, packet).context("Send packet")?;
            }
        }
    }

    Ok(())
}
