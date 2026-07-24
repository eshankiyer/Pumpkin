use std::sync::atomic::Ordering;

use crate::entity::EntityBase;
use pumpkin_data::{
    particle::Particle,
    sound::{Sound, SoundCategory},
};
use pumpkin_protocol::bedrock::server::animate::{AnimateAction, SAnimate};
use pumpkin_protocol::codec::var_ulong::VarULong;
use pumpkin_protocol::java::client::play::{Animation, CEntityAnimation};
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

impl AttackType {
    pub async fn new(player: &Player, attack_cooldown_progress: f32) -> Self {
        let entity = &player.get_entity();

        let sprinting = entity.is_sprinting();
        let on_ground = entity.on_ground.load(Ordering::Relaxed);
        let fall_distance = player.living_entity.fall_distance.load();
        let held_item = player.inventory().held_item();
        let is_mace = {
            let stack = held_item.lock().await;
            stack.item.id == pumpkin_data::item::Item::MACE.id
        };

        if is_mace && !on_ground && fall_distance > 1.5 {
            return Self::MaceSmash;
        }

        let sword = {
            let stack = held_item.lock().await;
            stack.is_sword()
        };

        let is_strong = attack_cooldown_progress > 0.9;
        if sprinting && is_strong {
            return Self::Knockback;
        }

        if is_strong && !on_ground && fall_distance > 0.0 {
            return Self::Critical;
        }

        if sword && is_strong {
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

/// Sends the critical-hit animations for a landed melee attack.
///
/// The vanilla client draws crit particles only when it receives these
/// packets, so the server has to send them; it does not predict them
/// locally, not even for the attacking player. Vanilla sends both for the
/// same hit when an enchanted weapon lands a crit.
pub fn send_crit_animations(
    world: &World,
    victim: &Entity,
    attack_type: AttackType,
    scaled_enchantment_damage: f64,
) {
    let chunk_pos = victim.chunk_pos.load();
    let entity_id = victim.entity_id;

    let send = |java: Animation, bedrock: AnimateAction| {
        world.broadcast_to_chunk_editioned_sync(
            chunk_pos,
            &CEntityAnimation::new(entity_id.into(), java),
            &SAnimate {
                action: bedrock,
                runtime_entity_id: VarULong(entity_id as u64),
                data: 0.0,
                swing_source: None,
            },
        );
    };

    if attack_type == AttackType::Critical {
        send(Animation::CriticalEffect, AnimateAction::CriticalHit);
    }
    if scaled_enchantment_damage > 0.0 {
        send(
            Animation::MagicCriticaleffect,
            AnimateAction::MagicCriticalHit,
        );
    }
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
