//! Robot Code for the MATE Sea Owls Team
#![feature(slice_as_chunks)]
#![warn(
    meta_variable_misuse,
    //missing_debug_implementations,
    //missing_docs,
    //unsafe_code,
    //unused_results,
    //unreachable_pub,
    //clippy::pedantic,
    //clippy::nursery,
    //clippy::unwrap_used,
    //clippy::expect_used
)]

pub mod peripheral;
mod systems;

use std::sync::{Arc, RwLock};
use common::state::RobotState;
use common::types::MotorId;
use crate::systems::motor::MotorSystem;
use crate::systems::networking::NetworkSystem;
use crate::systems::SystemManager;

fn main() -> anyhow::Result<()> {
    let robot = RobotState::new(
        &[
            MotorId::FrontL,
            MotorId::FrontR,
            MotorId::RearL,
            MotorId::RearR,
            MotorId::UpR,
            MotorId::UpL
        ]
    );
    let robot = Arc::new(RwLock::new(robot));

    let mut systems = SystemManager::new(robot.clone());

    systems.add_system::<NetworkSystem>()?;
    systems.add_system::<MotorSystem>()?;

    systems.start();

    Ok(())
}