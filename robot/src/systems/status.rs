use std::thread::Scope;

use common::{
    store::{tokens, Store, UpdateCallback},
    types::{Armed, Percent, RobotStatus},
};
use tracing::{span, Level};

use crate::{event::Event, events::EventHandle, SystemId};

use super::{motor, System};

pub struct StatusSystem;

impl System for StatusSystem {
    const ID: SystemId = SystemId::RobotStatus;

    fn start<'scope>(
        mut events: EventHandle,
        spawner: &'scope Scope<'scope, '_>,
    ) -> anyhow::Result<()> {
        let listener = events.take_listner().unwrap();

        spawner.spawn(move || {
            span!(Level::INFO, "Status manager");

            let mut store = Store::new(move |update| events.send(Event::Store(update)));
            let mut peers = 0;
            let mut last_status = None;

            for event in listener {
                let recompute_state = match &*event {
                    Event::PeerConnected(_) => {
                        peers += 1;
                        true
                    }
                    Event::PeerDisconnected(_) => {
                        peers -= 1;
                        true
                    }
                    Event::Store(update) => {
                        store.handle_update_shared(update);
                        true
                    }
                    Event::ResetForignStore => {
                        store.reset_shared();
                        true
                    }
                    Event::SyncStore => {
                        last_status = None;
                        true
                    }
                    Event::Error(_) => {
                        // TODO
                        true
                    }
                    Event::Exit => {
                        return;
                    }
                    _ => false,
                };

                if recompute_state {
                    let status = compute_status(&store, peers);

                    if last_status != Some(status) {
                        store.insert(&tokens::STATUS, status);

                        last_status = Some(status);
                    }
                }
            }
        });

        Ok(())
    }
}

fn compute_status<C: UpdateCallback>(store: &Store<C>, peers: i32) -> RobotStatus {
    if peers == 0 {
        return RobotStatus::NoPeer;
    }

    let mut state = RobotStatus::Disarmed;

    if let Some(armed) = store.get(&tokens::ARMED) {
        if matches!(*armed, Armed::Armed) {
            state = RobotStatus::Ready;

            if let Some(speeds) = store.get_alive(&tokens::MOTOR_SPEED, motor::MAX_UPDATE_AGE) {
                let max_speed = speeds
                    .values()
                    .map(|it| it.to_f64().abs())
                    .max_by(f64::total_cmp);
                if let Some(max_speed) = max_speed {
                    state = RobotStatus::Moving(Percent::new(max_speed));
                }
            }
        }
    }

    state
}
