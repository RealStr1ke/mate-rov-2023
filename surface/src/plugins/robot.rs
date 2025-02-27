use crate::plugins::networking::NetworkEvent;
use anyhow::anyhow;
use bevy::prelude::*;
use common::error::LogErrorExt;
use common::protocol::Protocol;
use common::store::adapters::{BackingType, TypeAdapter};
use common::store::{self, tokens, Key, Store, Token, Update, UpdateCallback};
use common::types::Armed;
use crossbeam::channel::{bounded, Receiver, Sender};
use fxhash::FxHashMap as HashMap;
use networking::error::NetError;
use std::any::Any;
use std::net::SocketAddr;
use std::time::SystemTime;

use super::notification::Notification;

pub struct RobotPlugin;

impl Plugin for RobotPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<RobotEvent>();
        app.add_event::<Update>();
        app.init_resource::<Robot>();
        app.init_resource::<Adapters>();
        app.add_system(update_robot.in_base_set(CoreSet::PreUpdate));
        app.add_system(updates_to_packets.in_base_set(CoreSet::PostUpdate));
        app.add_system(events_to_notifs.in_base_set(CoreSet::PostUpdate));
        app.add_system(arming_system.in_schedule(CoreSchedule::FixedUpdate));
    }
}

#[derive(Resource)]
pub struct Adapters(HashMap<Key, Box<dyn TypeAdapter<BackingType> + Send + Sync>>);

impl Default for Adapters {
    fn default() -> Self {
        Self(tokens::generate_adaptors())
    }
}

#[derive(Resource)]
pub struct Robot(
    Store<NotificationHandler>,
    Sender<Update>,
    Receiver<Update>,
    Armed,
);
impl Robot {
    pub const fn store(&self) -> &Store<NotificationHandler> {
        &self.0
    }

    pub fn store_mut(&mut self) -> &mut Store<NotificationHandler> {
        &mut self.0
    }

    pub fn arm(&mut self) {
        self.3 = Armed::Armed;
    }

    pub fn disarm(&mut self) {
        self.3 = Armed::Disarmed;
    }
}

impl Default for Robot {
    fn default() -> Self {
        let (tx, rx) = bounded(50);

        Self(
            Store::new(NotificationHandler(tx.clone())),
            tx,
            rx,
            Armed::Disarmed,
        )
    }
}

/// Way for systems to update store
/// For use with bevy's `Local` system argurment
pub struct Updater(Sender<Update>);
impl Updater {
    pub fn emit_update<V: Any + Send + Sync>(&self, token: &Token<V>, value: V) {
        let update = store::create_update(token, value);
        self.0.try_send(update).log_error("Emit update failed");
    }
    pub fn emit_delete<V: Any + Send + Sync>(&self, token: &Token<V>) {
        let update = store::create_delete(token);
        self.0.try_send(update).log_error("Emit delete failed");
    }
}

impl FromWorld for Updater {
    fn from_world(world: &mut World) -> Self {
        let robot = world.get_resource::<Robot>().expect("No `Robot` resource");

        Self(robot.1.clone())
    }
}

#[derive(Debug)]
pub enum RobotEvent {
    Store(Update),
    Ping(SystemTime, SystemTime),

    Connected(SocketAddr),
    Disconnected(SocketAddr),

    Error(NetError),
}

/// Handle `RobotEvent`s
fn update_robot(mut robot: ResMut<Robot>, mut events: EventReader<RobotEvent>) {
    for event in events.iter() {
        match event {
            RobotEvent::Store(update) => {
                robot.0.handle_update_shared(update);
            }
            RobotEvent::Connected(..) | RobotEvent::Disconnected(..) => {
                robot.0.reset();
            }
            _ => {}
        }
    }
}

/// Handle writes to store and send the corresponding packets to the robot
fn updates_to_packets(
    adapters: Res<Adapters>,
    mut robot: ResMut<Robot>,
    mut net: EventWriter<NetworkEvent>,
) {
    // Bypass rust ownership issue
    let robot = &mut *robot;

    for update in robot.2.try_iter() {
        robot.0.handle_update_owned(&update);

        let (key, data) = update;
        let adapter = adapters.0.get(&key);

        if let Some(adapter) = adapter {
            match data {
                Some(data) => {
                    let data = adapter.serialize(&*data);

                    if let Some(data) = data {
                        net.send(NetworkEvent::SendPacket(Protocol::Store(
                            key.into(),
                            Some(data),
                        )));
                    } else {
                        error!("Could not encode {}", key.as_str());
                    }
                }
                None => {
                    net.send(NetworkEvent::SendPacket(Protocol::Store(key.into(), None)));
                }
            }
        } else {
            error!("No adapter found for {}", key.as_str());
        }
    }
}

/// Generate notifications for some robot events
fn events_to_notifs(mut events: EventReader<RobotEvent>, mut notifs: EventWriter<Notification>) {
    for event in events.iter() {
        match event {
            RobotEvent::Connected(addr) => {
                notifs.send(Notification::Info(
                    "Robot Connected".to_owned(),
                    format!("Peer: {addr}"),
                ));
            }
            RobotEvent::Disconnected(addr) => {
                notifs.send(Notification::Info(
                    "Robot Disconnected".to_owned(),
                    format!("Peer: {addr}"),
                ));
            }
            RobotEvent::Error(error) => {
                notifs.send(Notification::Error(
                    "Network error".to_owned(),
                    anyhow!("{error}"),
                ));
            }
            RobotEvent::Store(store) => {
                if let Some(leak) = store::handle_update(&tokens::LEAK, store) {
                    if *leak {
                        notifs.send(Notification::Info(
                            "Leak Detected!".to_owned(),
                            "Take robot to surface!".to_owned(),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
}

fn arming_system(updater: Local<Updater>, robot: Option<ResMut<Robot>>) {
    if let Some(robot) = robot {
        updater.emit_update(&tokens::ARMED, robot.3);
    }
}

pub struct NotificationHandler(Sender<Update>);

impl UpdateCallback for NotificationHandler {
    fn call(&mut self, update: Update) {
        self.0
            .try_send(update)
            .log_error("NotificationHandler send failed");
    }
}
