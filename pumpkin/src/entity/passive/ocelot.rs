use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::meta_data_type::MetaDataType;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data::TrackedData;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        Controls, Goal, GoalFuture, active_target::ActiveTargetGoal, avoid_entity::AvoidEntityGoal,
        breed::BreedGoal, look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        ocelot_attack::OcelotAttackGoal, swim::SwimGoal, tempt::TemptGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

/// Contents of the `minecraft:ocelot_food` item tag, which vanilla `Ocelot.isFood` tests against.
const OCELOT_FOOD: &[&Item] = &[&Item::COD, &Item::SALMON];

/// Vanilla `Ocelot.CROUCH_SPEED_MOD` / `WALK_SPEED_MOD` / `SPRINT_SPEED_MOD`.
const CROUCH_SPEED_MOD: f64 = 0.6;
const WALK_SPEED_MOD: f64 = 0.8;
const SPRINT_SPEED_MOD: f64 = 1.33;

/// Represents an Ocelot, a shy passive mob found in jungles.
///
/// Wiki: <https://minecraft.wiki/w/Ocelot>
pub struct OcelotEntity {
    pub mob_entity: MobEntity,
    /// Vanilla `Ocelot.DATA_TRUSTING`, persisted as the `Trusting` NBT flag.
    trusting: Arc<AtomicBool>,
    /// Whether the tempt goal is currently running; vanilla `mobInteract` gates the taming
    /// attempt on `temptGoal.isRunning()`.
    tempt_running: Arc<AtomicBool>,
}

impl OcelotEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let trusting = Arc::new(AtomicBool::new(false));
        let tempt_running = Arc::new(AtomicBool::new(false));
        let ocelot = Self {
            mob_entity,
            trusting: trusting.clone(),
            tempt_running: tempt_running.clone(),
        };
        let mob_arc = Arc::new(ocelot);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut goal_selector = mob_arc.mob_entity.goals_selector.lock().unwrap();

            // Priorities and speeds follow vanilla Ocelot.registerGoals().
            goal_selector.add_goal(1, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                3,
                Box::new(OcelotTemptGoal {
                    inner: TemptGoal::new(CROUCH_SPEED_MOD, OCELOT_FOOD),
                    running: tempt_running,
                }),
            );
            goal_selector.add_goal(
                4,
                Box::new(OcelotAvoidPlayersGoal {
                    inner: AvoidEntityGoal::new(
                        &EntityType::PLAYER,
                        16.0,
                        WALK_SPEED_MOD,
                        SPRINT_SPEED_MOD,
                    ),
                    trusting,
                }),
            );
            goal_selector.add_goal(8, Box::new(OcelotAttackGoal::new()));
            goal_selector.add_goal(9, BreedGoal::new(0.8));
            goal_selector.add_goal(10, Box::new(WanderAroundGoal::new(WALK_SPEED_MOD)));
            goal_selector.add_goal(
                11,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 10.0),
            );
            goal_selector.add_goal(11, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc.mob_entity.target_selector.lock().unwrap();
            target_selector.add_goal(
                1,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::CHICKEN, true),
            );
            target_selector.add_goal(
                1,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::TURTLE, true),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn is_trusting(&self) -> bool {
        self.trusting.load(Ordering::Relaxed)
    }

    fn set_trusting(&self, trusting: bool) {
        self.trusting.store(trusting, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                TrackedData::TRUSTING,
                MetaDataType::BOOLEAN,
                trusting,
            )],
            None,
        );
    }
}

impl Animal for OcelotEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        OCELOT_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl NBTStorage for OcelotEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_animal_nbt(nbt);
            nbt.put_bool("Trusting", self.is_trusting());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_animal_nbt(nbt);
            self.trusting
                .store(nbt.get_bool("Trusting").unwrap_or(false), Ordering::Relaxed);
        })
    }
}

impl Mob for OcelotEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            // Vanilla Ocelot.mobInteract: while untrusting, feeding ocelot food within 3 blocks
            // has a 1-in-3 chance of making the ocelot trust the player. This branch consumes the
            // item and returns before the Animal breeding path, so an untrusting ocelot never
            // enters love mode.
            let entity = &self.mob_entity.living_entity.entity;
            let dist_sq = player
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&entity.pos.load());

            if self.tempt_running.load(Ordering::Relaxed)
                && !self.is_trusting()
                && self.is_food(item_stack)
                && dist_sq < 9.0
            {
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);

                let world = entity.world.load();
                if self.get_random().random_range(0..3) == 0 {
                    self.set_trusting(true);
                    world.send_entity_status(entity, EntityStatus::TrustingSucceeded);
                } else {
                    world.send_entity_status(entity, EntityStatus::TrustingFailed);
                }

                return true;
            }

            self.animal_interact(player, item_stack, Sound::EntityOcelotAmbient)
                .await
        })
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            if entity.age.load(Ordering::Relaxed) < 0 {
                entity.send_meta_data(
                    &[Metadata::new(
                        TrackedData::BABY_ID,
                        MetaDataType::BOOLEAN,
                        true,
                    )],
                    None,
                );
            }
            entity.send_meta_data(
                &[Metadata::new(
                    TrackedData::TRUSTING,
                    MetaDataType::BOOLEAN,
                    self.is_trusting(),
                )],
                None,
            );
        })
    }
}

/// Vanilla `Ocelot.OcelotTemptGoal` overrides `canScare` so a trusting ocelot is not scared off.
/// Pumpkin's `TemptGoal` has no scare handling, so this wrapper only tracks whether the goal is
/// running, which vanilla's `mobInteract` gates the taming attempt on.
struct OcelotTemptGoal {
    inner: TemptGoal,
    running: Arc<AtomicBool>,
}

impl Goal for OcelotTemptGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.inner.can_start(mob)
    }

    fn should_continue<'a>(&'a self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.inner.should_continue(mob)
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.running.store(true, Ordering::Relaxed);
        self.inner.start(mob)
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.running.store(false, Ordering::Relaxed);
        self.inner.stop(mob)
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.tick(mob)
    }

    fn should_run_every_tick(&self) -> bool {
        self.inner.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}

/// Vanilla `Ocelot.OcelotAvoidEntityGoal`: an untrusting ocelot flees players, with both
/// `canUse` and `canContinueToUse` gated on `!isTrusting()`.
struct OcelotAvoidPlayersGoal {
    inner: AvoidEntityGoal,
    trusting: Arc<AtomicBool>,
}

impl Goal for OcelotAvoidPlayersGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        if self.trusting.load(Ordering::Relaxed) {
            return Box::pin(async { false });
        }
        self.inner.can_start(mob)
    }

    fn should_continue<'a>(&'a self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        if self.trusting.load(Ordering::Relaxed) {
            return Box::pin(async { false });
        }
        self.inner.should_continue(mob)
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.start(mob)
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.stop(mob)
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.inner.tick(mob)
    }

    fn should_run_every_tick(&self) -> bool {
        self.inner.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}
