use std::sync::atomic::Ordering;

use crate::entity::EntityBase;
use pumpkin_data::{
    particle::Particle,
    sound::{Sound, SoundCategory},
};
use pumpkin_util::math::vector3::Vector3;

use crate::{
    entity::{Entity, player::Player},
    world::World,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackType {
    Knockback,
    Critical,
    Sweeping,
    Strong,
    Weak,
    MaceSmash,
}

#[derive(Clone, Copy)]
struct CriticalAttackContext {
    full_strength: bool,
    on_ground: bool,
    fall_distance: f32,
    mobility_restricted: bool,
    climbing: bool,
    touching_water: bool,
    has_vehicle: bool,
    sprinting: bool,
    target_is_living: bool,
}

const fn can_critical_attack(context: CriticalAttackContext) -> bool {
    context.full_strength
        && !context.on_ground
        && context.fall_distance > 0.0
        && !context.mobility_restricted
        && !context.climbing
        && !context.touching_water
        && !context.has_vehicle
        && !context.sprinting
        && context.target_is_living
}

impl AttackType {
    pub async fn new(
        player: &Player,
        attack_cooldown_progress: f32,
        target_is_living: bool,
    ) -> Self {
        let entity = &player.get_entity();

        let sprinting = entity.is_sprinting();
        let on_ground = entity.on_ground.load(Ordering::Relaxed);
        let fall_distance = player.living_entity.fall_distance.load();
        let held_item = player.inventory().held_item();
        let is_mace = {
            let stack = held_item.lock().await;
            stack.item.id == pumpkin_data::item::Item::MACE.id
        };

        let mobility_restricted = player
            .living_entity
            .has_effect(&pumpkin_data::effect::StatusEffect::BLINDNESS)
            .await;
        let has_vehicle = entity.has_vehicle().await;

        if is_mace && !on_ground && fall_distance > 1.5 {
            return Self::MaceSmash;
        }

        let sword = {
            let stack = held_item.lock().await;
            stack.is_sword()
        };
        let max_sweep_speed = player
            .living_entity
            .get_attribute_value(&pumpkin_data::attributes::Attributes::MOVEMENT_SPEED)
            * 2.5;
        let sweep_speed_ok =
            entity.velocity.load().horizontal_length_squared() < max_sweep_speed * max_sweep_speed;

        let is_strong = attack_cooldown_progress > 0.9;
        if sprinting && is_strong {
            return Self::Knockback;
        }

        if can_critical_attack(CriticalAttackContext {
            full_strength: is_strong,
            on_ground,
            fall_distance,
            mobility_restricted,
            climbing: player.living_entity.climbing.load(Ordering::Relaxed),
            touching_water: entity.touching_water.load(Ordering::Relaxed),
            has_vehicle,
            sprinting,
            target_is_living,
        }) {
            return Self::Critical;
        }

        if sword && is_strong && on_ground && sweep_speed_ok {
            return Self::Sweeping;
        }

        if is_strong { Self::Strong } else { Self::Weak }
    }
}

pub fn handle_knockback(attacker: &Entity, victim: &Entity, strength: f64) {
    let yaw = attacker.yaw.load();
    victim.knockback(
        strength * 0.5,
        f64::from((yaw.to_radians()).sin()),
        f64::from(-(yaw.to_radians()).cos()),
    );

    let velocity = attacker.velocity.load();
    attacker.velocity.store(velocity.multiply(0.6, 1.0, 0.6));
}

pub fn spawn_sweep_particle(attacker_entity: &Entity, world: &World, pos: &Vector3<f64>) {
    let yaw = attacker_entity.yaw.load();
    let d = -f64::from((yaw.to_radians()).sin());
    let e = f64::from((yaw.to_radians()).cos());

    let scale = 0.5;
    let body_y = f64::from(attacker_entity.height()).mul_add(scale, pos.y);

    world.spawn_particle(
        Vector3::new(pos.x + d, body_y, pos.z + e),
        Vector3::new(0.0, 0.0, 0.0),
        0.0,
        0,
        Particle::SweepAttack,
    );
}

pub async fn player_attack_sound(pos: &Vector3<f64>, world: &World, attack_type: AttackType) {
    match attack_type {
        AttackType::Knockback => {
            world.play_sound(
                Sound::EntityPlayerAttackKnockback,
                SoundCategory::Players,
                pos,
            );
        }
        AttackType::Critical => {
            world.play_sound(Sound::EntityPlayerAttackCrit, SoundCategory::Players, pos);
        }
        AttackType::Sweeping => {
            world.play_sound(Sound::EntityPlayerAttackSweep, SoundCategory::Players, pos);
        }
        AttackType::Strong => {
            world.play_sound(Sound::EntityPlayerAttackStrong, SoundCategory::Players, pos);
        }
        AttackType::Weak => {
            world.play_sound(Sound::EntityPlayerAttackWeak, SoundCategory::Players, pos);
        }
        AttackType::MaceSmash => {
            world.play_sound(Sound::ItemMaceSmashAir, SoundCategory::Players, pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CriticalAttackContext, can_critical_attack};

    fn context() -> CriticalAttackContext {
        CriticalAttackContext {
            full_strength: true,
            on_ground: false,
            fall_distance: 1.0,
            mobility_restricted: false,
            climbing: false,
            touching_water: false,
            has_vehicle: false,
            sprinting: false,
            target_is_living: true,
        }
    }

    #[test]
    fn critical_attacks_require_vanilla_mobility_conditions() {
        assert!(can_critical_attack(context()));

        for blocked in [
            CriticalAttackContext {
                mobility_restricted: true,
                ..context()
            },
            CriticalAttackContext {
                climbing: true,
                ..context()
            },
            CriticalAttackContext {
                touching_water: true,
                ..context()
            },
            CriticalAttackContext {
                sprinting: true,
                ..context()
            },
            CriticalAttackContext {
                target_is_living: false,
                ..context()
            },
        ] {
            assert!(!can_critical_attack(blocked));
        }
    }
}
