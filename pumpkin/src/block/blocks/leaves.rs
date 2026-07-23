use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, GetStateForNeighborUpdateArgs, OnPlaceArgs,
    OnScheduledTickArgs, RandomTickArgs,
};
use pumpkin_data::block_properties::{BlockProperties, OakLeavesLikeProperties};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection, BlockId, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

use crate::world::World;

pub struct LeavesBlock;

impl BlockMetadata for LeavesBlock {
    fn ids() -> Box<[BlockId]> {
        tag::get_tag_values(tag::RegistryKey::Block, "minecraft:leaves")
            .unwrap_or_default()
            .iter()
            .filter_map(|key| {
                Block::from_registry_key(key.strip_prefix("minecraft:").unwrap_or(key))
            })
            .map(|block| block.id)
            .collect()
    }
}

/// The distance value another block contributes to a neighboring leaves
/// block: logs are the source (`0`), leaves pass on their own distance, and
/// any other block contributes nothing.
fn distance_from_state(block: &'static Block, state_id: BlockStateId) -> Option<u8> {
    if block.has_tag(&tag::Block::MINECRAFT_LOGS) {
        return Some(0);
    }
    block
        .has_tag(&tag::Block::MINECRAFT_LEAVES)
        .then(|| OakLeavesLikeProperties::from_state_id(state_id, block).distance)
}

/// Computes the `distance` property for leaves at `position`:
/// one more than the smallest neighbor distance, capped at 7.
fn compute_distance(world: &World, position: &BlockPos) -> u8 {
    let mut distance = 7u8;
    for direction in BlockDirection::all() {
        let neighbor = position.offset(direction.to_offset());
        let (block, state) = world.get_block_and_state(&neighbor);
        if let Some(neighbor_distance) = distance_from_state(block, state.id) {
            distance = distance.min(neighbor_distance.saturating_add(1));
        }
    }
    distance.max(1)
}

impl BlockBehaviour for LeavesBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props =
                OakLeavesLikeProperties::from_state_id(args.block.default_state.id, args.block);
            // Player-placed leaves never decay, matching vanilla.
            props.persistent = true;
            props.distance = compute_distance(args.world, args.position);
            props.to_state_id(args.block)
        })
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            // Defer the recalculation to a scheduled tick like vanilla, so
            // cascading updates through a canopy stay cheap.
            args.world
                .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
            args.state_id
        })
    }

    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let mut props = OakLeavesLikeProperties::from_state_id(state_id, args.block);
            let distance = compute_distance(args.world, args.position);
            if props.distance != distance {
                props.distance = distance;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
            }
        })
    }

    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let props = OakLeavesLikeProperties::from_state_id(state_id, args.block);
            // Vanilla decay: leaves too far from any log break on random tick.
            if !props.persistent && props.distance >= 7 {
                args.world
                    .break_block(args.position, None, BlockFlags::NOTIFY_ALL)
                    .await;
            }
        })
    }
}
