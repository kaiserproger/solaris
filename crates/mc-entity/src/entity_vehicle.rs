use std::collections::{HashMap, HashSet};

use crate::{
    EntityId, EntityInputCommand, EntityKinematics, EntityLifecycle, EntitySnapshot, EntityStage,
    EntityStore, VehicleError, VehicleInput, VehicleKind, VehicleState,
};

impl EntityStore {
    pub(super) fn vehicle_graph_accepts(&self, pending: &[EntitySnapshot]) -> bool {
        let mut entities = self
            .runtime
            .vehicle_states()
            .map(|(id, lifecycle, vehicle)| (id, (lifecycle, vehicle)))
            .collect::<HashMap<_, _>>();
        entities.extend(
            pending
                .iter()
                .map(|entity| (entity.id, (entity.lifecycle, entity.vehicle))),
        );
        let mut passenger_owners = HashMap::new();
        for (&id, &(lifecycle, vehicle)) in &entities {
            let Some(vehicle) = vehicle else {
                continue;
            };
            if lifecycle != EntityLifecycle::Alive {
                return false;
            }
            let Some(passenger) = vehicle.passenger else {
                continue;
            };
            if passenger == id
                || entities
                    .get(&passenger)
                    .is_none_or(|(lifecycle, _)| *lifecycle != EntityLifecycle::Alive)
                || passenger_owners.insert(passenger, id).is_some()
            {
                return false;
            }
        }
        let mut visited = HashSet::new();
        for &start in entities.keys() {
            let mut current = Some(start);
            visited.clear();
            while let Some(id) = current {
                if !visited.insert(id) {
                    return false;
                }
                current = entities
                    .get(&id)
                    .and_then(|(_, vehicle)| *vehicle)
                    .and_then(|vehicle| vehicle.passenger);
            }
        }
        true
    }

    pub fn mount_vehicle(
        &mut self,
        vehicle: EntityId,
        passenger: EntityId,
    ) -> Result<(), VehicleError> {
        if vehicle == passenger {
            return Err(VehicleError::SelfMount);
        }
        let vehicle_snapshot = self.snapshot(vehicle).ok_or(VehicleError::MissingVehicle)?;
        let passenger_snapshot = self
            .snapshot(passenger)
            .ok_or(VehicleError::MissingPassenger)?;
        if vehicle_snapshot.lifecycle != EntityLifecycle::Alive
            || passenger_snapshot.lifecycle != EntityLifecycle::Alive
        {
            return Err(VehicleError::InvalidLifecycle);
        }
        if self.vehicle_for_passenger(passenger).is_some() {
            return Err(VehicleError::PassengerAlreadyMounted);
        }
        if self.passenger_chain_contains(passenger, vehicle) {
            return Err(VehicleError::Cycle);
        }
        let mut state = vehicle_snapshot.vehicle.ok_or(VehicleError::NotVehicle)?;
        if state.passenger.is_some() {
            return Err(VehicleError::AlreadyMounted);
        }
        state.passenger = Some(passenger);
        self.set_vehicle_state(vehicle, Some(state));
        Ok(())
    }

    pub fn dismount_vehicle(
        &mut self,
        vehicle: EntityId,
        passenger: EntityId,
    ) -> Result<(), VehicleError> {
        let mut state = self
            .snapshot(vehicle)
            .ok_or(VehicleError::MissingVehicle)?
            .vehicle
            .ok_or(VehicleError::NotVehicle)?;
        if state.passenger != Some(passenger) {
            return Err(VehicleError::PassengerMismatch);
        }
        state.passenger = None;
        self.set_vehicle_state(vehicle, Some(state));
        Ok(())
    }

    pub fn apply_vehicle_input(
        &mut self,
        vehicle: EntityId,
        passenger: EntityId,
        input: VehicleInput,
    ) -> Result<(), VehicleError> {
        let vehicle_snapshot = self.snapshot(vehicle).ok_or(VehicleError::MissingVehicle)?;
        let passenger_snapshot = self
            .snapshot(passenger)
            .ok_or(VehicleError::MissingPassenger)?;
        if vehicle_snapshot.lifecycle != EntityLifecycle::Alive
            || passenger_snapshot.lifecycle != EntityLifecycle::Alive
        {
            return Err(VehicleError::InvalidLifecycle);
        }
        let state = vehicle_snapshot.vehicle.ok_or(VehicleError::NotVehicle)?;
        if state.passenger != Some(passenger) {
            return Err(VehicleError::PassengerMismatch);
        }
        let mut rotation = vehicle_snapshot.rotation;
        let mut velocity = vehicle_snapshot.velocity;
        match state.kind {
            VehicleKind::Boat => {
                let yaw_delta = match (input.left, input.right) {
                    (true, false) => -4.0,
                    (false, true) => 4.0,
                    _ => 0.0,
                };
                rotation.yaw += yaw_delta;
                rotation.head_yaw = rotation.yaw;

                let speed = match (input.forward, input.backward) {
                    (true, false) => 0.35,
                    (false, true) => -0.12,
                    _ => 0.0,
                };
                let radians = f64::from(rotation.yaw).to_radians();
                velocity.x = -radians.sin() * speed;
                velocity.z = radians.cos() * speed;
                velocity.y = 0.0;
            }
            VehicleKind::Minecart => return Err(VehicleError::UnsupportedSteering),
        }
        self.apply_kinematics([EntityKinematics {
            id: vehicle,
            position: vehicle_snapshot.position,
            rotation,
            velocity,
            on_ground: vehicle_snapshot.on_ground,
        }]);
        Ok(())
    }

    #[must_use]
    pub fn vehicle_for_passenger(&self, passenger: EntityId) -> Option<EntityId> {
        self.runtime
            .vehicle_states()
            .find_map(|(id, _, vehicle)| (vehicle?.passenger == Some(passenger)).then_some(id))
    }

    fn passenger_chain_contains(&self, start: EntityId, target: EntityId) -> bool {
        let mut current = Some(start);
        for _ in 0..self.len() {
            let Some(id) = current else {
                return false;
            };
            if id == target {
                return true;
            }
            let Some(snapshot) = self.snapshot(id) else {
                return false;
            };
            current = snapshot.vehicle.and_then(|state| state.passenger);
        }
        false
    }

    pub(super) fn set_vehicle_state(
        &mut self,
        id: EntityId,
        vehicle: Option<VehicleState>,
    ) -> bool {
        if self.is_runtime_entity(id) {
            self.runtime
                .queue_input(EntityInputCommand::SetVehicle { id, vehicle });
            self.runtime.run_stage(EntityStage::InputAi);
            return true;
        }

        false
    }

    pub(super) fn sanitized_snapshot_vehicle(
        &self,
        id: EntityId,
        lifecycle: EntityLifecycle,
        vehicle: Option<VehicleState>,
    ) -> Option<VehicleState> {
        if lifecycle != EntityLifecycle::Alive {
            return None;
        }
        let state = vehicle?;
        let Some(passenger) = state.passenger else {
            return Some(state);
        };
        if passenger == id || self.vehicle_for_passenger(passenger).is_some() {
            return None;
        }

        let mut current = Some(passenger);
        for _ in 0..=self.len() {
            let Some(current_id) = current else {
                return Some(state);
            };
            if current_id == id {
                return None;
            }
            let current_snapshot = self.snapshot(current_id)?;
            if current_snapshot.lifecycle != EntityLifecycle::Alive {
                return None;
            }
            current = current_snapshot.vehicle.and_then(|state| state.passenger);
        }
        None
    }
}
