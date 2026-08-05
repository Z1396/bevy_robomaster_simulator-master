use std::collections::{HashMap, VecDeque};

use bevy::{
    camera::visibility::Visibility,
    ecs::{
        bundle::Bundle,
        entity::Entity,
        hierarchy::Children,
        system::{Commands, Query},
    },
    platform::collections::HashSet,
};

pub fn drain_entities_by<T, F: Fn(&T) -> bool>(
    name_map: &mut HashMap<T, Entity>,
    predicate: F,
) -> Vec<Entity> {
    name_map
        .extract_if(|k, _v| predicate(k))
        .map(|v| v.1)
        .collect()
}

pub fn insert_all_child<B: Bundle + Clone, F: Fn() -> B>(
    commands: &mut Commands,
    root: Entity,
    query: &Query<&Children>,
    bundle: F,
) {
    let mut set = HashSet::new();
    let mut stack = VecDeque::new();
    stack.push_back(root);

    while let Some(entity) = stack.pop_front() {
        if !set.insert(entity) {
            continue;
        }
        commands.entity(entity).insert(bundle());
        if let Ok(children) = query.get(entity) {
            stack.extend(children.iter().copied());
        }
    }
}

pub fn set_visibility(
    entity: Entity,
    value: Visibility,
    visibilities: &mut Query<&mut Visibility>,
) -> Result<(), bevy::ecs::query::QueryEntityError> {
    let mut visibility = visibilities.get_mut(entity)?;
    *visibility = value;
    Ok(())
}
