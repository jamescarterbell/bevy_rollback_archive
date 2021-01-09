#![feature(trait_alias)]

use bevy::{
    ecs::{Schedule, Stage, ShouldRun},
    prelude::{
        *,
        stage::UPDATE,
    },
    reflect::TypeRegistryArc,
    scene::serde::SceneSerializer,
};
use std::sync::Mutex;
use std::ops::DerefMut;
use std::collections::hash_map::*;

pub mod stage{
    pub const ROLLBACK_UPDATE: &str = "rollback_update";
}

pub mod logic_stage{
    pub const SAVE_STATE: &str = "save_state";
}

pub struct RollbackPlugin{
    schedule: Mutex<Option<Schedule>>,
    buffer_size: usize,
    run_criteria: Mutex<Option<Box<dyn System<In = (), Out = ShouldRun>>>>,
}

impl RollbackPlugin{
    pub fn new(buffer_size: usize, schedule: Schedule) -> Self{
        Self{
            schedule: Mutex::new(Some(schedule)),
            buffer_size,
            run_criteria: Mutex::new(None),
        }
    }

    pub fn with_buffer_size(buffer_size: usize) -> Self{
        Self{
            schedule: Mutex::new(None),
            buffer_size,
            run_criteria: Mutex::new(None),
        }
    }

    pub fn with_run_criteria<S: System<In = (), Out = ShouldRun>>(mut self, system: S) -> Self {
        self.run_criteria = Mutex::new(Some(Box::new(system)));
        self
    }
}

impl Plugin for RollbackPlugin{
    fn build(&self, app: &mut AppBuilder){
        let mut resources = app.resources_mut();

        let rollback_buffer = RollbackBuffer::new(
            self.buffer_size,
            &mut resources.get_mut::<Assets<DynamicScene>>().expect("Couldn't find DynamicScene!"),
            &resources.get::<TypeRegistryArc>().expect("Couldn't find TypeRegistryArc"),
        );

        drop(resources);

        let run_criteria = self.run_criteria.lock().unwrap().take();

        let mut stage = RollbackStage::with_schedule(self.schedule.lock().unwrap().take().unwrap());
        
        stage.run_criteria = run_criteria;
        stage.run_criteria_initialized = false;

        app
            .add_resource(
                rollback_buffer
            )
            .add_stage_before(
                UPDATE,
                stage::ROLLBACK_UPDATE,
                stage
            );
    }
}

pub struct RollbackStage{
    schedule: Schedule,
    run_criteria: Option<Box<dyn System<In = (), Out = ShouldRun>>>,
    run_criteria_initialized: bool,
}

impl RollbackStage{
    pub fn with_schedule(schedule: Schedule) -> Self{
        Self{
            schedule,
            run_criteria: None,
            run_criteria_initialized: false,
        }
    }

    pub fn new() -> Self{
        Self{
            schedule: Schedule::default(),
            run_criteria: None,
            run_criteria_initialized: false,
        }
    }

    pub fn run_once(&mut self, world: &mut World, resources: &mut Resources){
        // Update tracked entities
        update_tracked_entities(world, resources);
        // Update tracked resources
        self.schedule.run_once(world, resources);
    }

    pub fn run_rollback(&mut self, world: &mut World, resources: &mut Resources){
        loop{
            
            let current_state = resources
                .get::<RollbackBuffer>()
                .expect("Couldn't find RollbackBuffer!")
                .rollback_state;

            match current_state{
                RollbackState::Rollback(state) => {
                    // Perform initial rollback
                    // Despawn current rollback scene
                    // Spawn new rollback scene
                    let mut scene_spawner = resources
                        .get_mut::<SceneSpawner>()
                        .expect("Couldn't find SceneSpawner!");

                    let mut rollback_buffer = resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!");

                    let type_registry = resources
                        .get::<TypeRegistryArc>()
                        .expect("Couldn't find TypeRegistryArc");

                    let mut assets = resources
                        .get_mut::<Assets<DynamicScene>>()
                        .expect("Couldn't find DynamicScenes");

                    let old_scene = rollback_buffer
                        .tracked_entities
                        .clone();

                    scene_spawner.despawn(old_scene);

                    let len = rollback_buffer.scenes.len();

                    let new_scene = DynamicScene::from_scene(rollback_buffer
                        .scenes
                        .get(state % len)
                        .expect("Couldn't find scene in buffer!"),
                        &type_registry);

                    let new_scene = assets.add(new_scene);

                    scene_spawner.spawn_dynamic(new_scene.clone());
                    rollback_buffer.tracked_entities = new_scene;

                    scene_spawner.despawn_queued_scenes(world);
                    scene_spawner.spawn_queued_scenes(world, resources);

                    // Setup for catchup
                    rollback_buffer
                        .rollback_state = RollbackState::Rolledback(state);

                    // Perform resource changes last since we'll have to drop everything we were using
                    let resource_rollback = rollback_buffer.resource_rollback_fn.take().unwrap_or(Vec::new());
                    let past_resources = rollback_buffer
                        .resources.get_mut(state % len)
                        .expect("Couldn't find resources in buffer!")
                        .take()
                        .unwrap_or(Resources::default());

                    drop(scene_spawner);
                    drop(rollback_buffer);
                    drop(type_registry);
                    drop(assets);

                    for resource_rollback_fn in resource_rollback.iter(){
                        (resource_rollback_fn)(resources, &past_resources);
                    }

                    resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!")
                        .resource_rollback_fn = Some(resource_rollback);
                },
                RollbackState::Rolledback(state) => {
                    // Apply overrides to state from stored state (for inputs for instance)
                    let mut rollback_buffer = resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!");

                    let len = rollback_buffer.scenes.len();
                    let resource_overrides = rollback_buffer.resource_rollback_fn.take().unwrap_or(Vec::new());

                    let past_resources = rollback_buffer
                        .resources.get_mut(state % len)
                        .expect("Couldn't find resources in buffer!")
                        .take()
                        .unwrap_or(Resources::default());

                    drop(rollback_buffer);

                    for resource_overrides_fn in resource_overrides.iter(){
                        (resource_overrides_fn)(resources, &past_resources);
                    }
                    
                    let mut rollback_buffer = resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!");

                    rollback_buffer
                        .resource_rollback_fn = Some(resource_overrides);

                    // Apply buffered changes for state

                    drop(rollback_buffer);
                    
                    let changes = resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!")
                        .buffered_changes
                        .remove(&state);

                    if let Some(mut changes) = changes{
                        changes.run_once(world, resources);
                    }

                    // Run schedule for state_n
                    self.run_once(world, resources);
                    
                    let mut rollback_buffer = resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!");
            
                    // Increment counters
                    let new_state = state + 1;
                    rollback_buffer.rollback_state = RollbackState::Rolledback(new_state);
                    
                    let type_registry = resources
                        .get::<TypeRegistryArc>()
                        .expect("Couldn't find TypeRegistryArc!");

                    let assets = resources
                        .get::<Assets<DynamicScene>>()
                        .expect("Couldn't find DynamicScene Assets");
                
                    // Store state_n+1
                    let stored_scene = assets
                        .get(rollback_buffer.tracked_entities.clone())
                        .expect("Couldn't find rollback scene!")
                        .get_scene(resources)
                        .expect("Couldn't get Scene from DynamicScene!");


                    *rollback_buffer
                        .scenes
                        .get_mut(state % len)
                        .expect("Index error in scene buffer!") = stored_scene;

                    let mut stored_resources = Resources::default();

                    let resource_rollback = rollback_buffer.resource_rollback_fn.take().unwrap_or(Vec::new());
                    for resource_rollback_fn in resource_rollback.iter(){
                        (resource_rollback_fn)(&mut stored_resources, &resources);
                    }

                    resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!")
                        .resource_rollback_fn = Some(resource_rollback);

                    if new_state == rollback_buffer.newest_frame{
                        // We're all caugt up!
                        break;
                    }
                }
            };
        };
    }

    pub fn with_run_criteria<S: System<In = (), Out = ShouldRun>>(mut self, system: S) -> Self {
        self.set_run_criteria(system);
        self
    }

    pub fn set_run_criteria<S: System<In = (), Out = ShouldRun>>(
        &mut self,
        system: S,
    ) -> &mut Self {
        self.run_criteria = Some(Box::new(system.system()));
        self.run_criteria_initialized = false;
        self
    }
}

impl Stage for RollbackStage{
    fn initialize(&mut self, world: &mut World, resources: &mut Resources){
        if let Some(ref mut run_criteria) = self.run_criteria{
            if !self.run_criteria_initialized{
                run_criteria.initialize(world, resources);
                self.run_criteria_initialized = true;
            }
        }
        self.schedule.initialize(world, resources);
    }

    fn run(&mut self, world: &mut World, resources: &mut Resources){
        loop{
            // Check timestep condition
            let should_run = if let Some(ref mut run_criteria) = self.run_criteria{
                let should_run = run_criteria.run((), world, resources);
                run_criteria.run_thread_local(world, resources);
                should_run.unwrap_or(ShouldRun::No)
            } else {
                ShouldRun::No
            };
            // Check rollback condition during the fixed time step
            match should_run{
                ShouldRun::No => return,
                ShouldRun::Yes=>{
                    resources.get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!").newest_frame += 1;
                    self.run_rollback(world, resources);
                    return;
                }
                ShouldRun::YesAndLoop => {
                    resources.get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!").newest_frame += 1;
                    self.run_rollback(world, resources);
                    continue;
                },
            }   
        }
    }
}

#[derive(Copy, Clone)]
enum RollbackState{
    // Rollback to this state
    Rollback(usize),
    // Currently rolledback to this state, this could be the newest frame which is okay
    Rolledback(usize),
}

#[derive(Debug)]
pub enum RollbackError{
    FrameTimeout,
    ResourceNotFound,
}

pub trait ResourceRollbackFn = Fn(&mut Resources, &Resources) -> () + Sync + Send;

pub struct RollbackBuffer{
    newest_frame: usize,
    rollback_state: RollbackState,
    tracked_entities: Handle<DynamicScene>,

    buffered_changes: HashMap<usize, SystemStage>,
    scenes: Vec<Scene>,
    resources: Vec<Option<Resources>>,   

    resource_rollback_fn: Option<Vec<Box<dyn ResourceRollbackFn>>>,
    resource_overrides: Option<Vec<Box<dyn ResourceRollbackFn>>>
}

impl RollbackBuffer{
    pub fn new(buffer_size: usize, assets: &mut Assets<DynamicScene>, type_registry: &TypeRegistryArc) -> Self{
        RollbackBuffer{
            newest_frame: 0,
            rollback_state: RollbackState::Rolledback(0),
            tracked_entities: assets
                .add(DynamicScene::from_world(
                    &World::new(),
                    type_registry
            )),

            buffered_changes: HashMap::new(),
            scenes: Vec::with_capacity(buffer_size),
            resources: Vec::with_capacity(buffer_size),

            resource_rollback_fn: None,
            resource_overrides: None
        }
    }

    pub fn past_frame_change<S: System<In = (), Out = ()>>(&mut self, op: S, frame: usize) -> Result<(), RollbackError>{
        if self.newest_frame - frame >= self.scenes.len(){
            return Err(RollbackError::FrameTimeout);
        }
        match self.buffered_changes.entry(frame){
            Entry::Occupied(mut o) => o.get_mut().add_system(op),
            Entry::Vacant(v) => v.insert({
                let mut stage = SystemStage::parallel();
                stage.add_system(op);
                stage
            }),
        };
        Ok(())
    }
}

struct RollbackTracked;

fn update_tracked_entities(world: &mut World, resources: &mut Resources){
    let mut scene = DynamicScene::default();
    let type_registry_arc = resources.get::<TypeRegistryArc>().expect("Couldn't find TypeRegistryArc");
    let type_registry = type_registry_arc.read();

    for archetype in world.archetypes().filter(|at| at.has::<RollbackTracked>()){
        let mut entities = Vec::new();
        for (index, entity) in archetype.iter_entities().enumerate() {
            if index == entities.len() {
                entities.push(bevy::scene::Entity {
                    entity: entity.id(),
                    components: Vec::new(),
                })
            }
            for type_info in archetype.types() {
                if let Some(registration) = type_registry.get(type_info.id()) {
                    if let Some(reflect_component) = registration.data::<ReflectComponent>() {
                        // SAFE: the index comes directly from a currently live component
                        unsafe {
                            let component =
                                reflect_component.reflect_component(&archetype, index);
                            entities[index].components.push(component.clone_value());
                        }
                    }
                }
            }
        }

        scene.entities.extend(entities.drain(..));
    }

    let mut rollback_buffer = resources.get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!");
    let mut scenes = resources.get_mut::<Assets<DynamicScene>>().expect("Couldn't find Dynamic Scene Assets!");

    scenes.remove(rollback_buffer.tracked_entities.clone());
    rollback_buffer.tracked_entities = scenes.add(scene);
}

pub trait ResourceTracker{
    fn track_resource<R: Resource + Clone +>(&mut self) -> &mut Self;
    fn override_resouce<R: Resource + Clone>(&mut self) -> &mut Self;
}

impl ResourceTracker for AppBuilder{
    fn track_resource<R: Resource + Clone>(&mut self) -> &mut Self{
        {
            let mut rollback_buffer = self.resources().get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!");

            rollback_buffer
                .resource_rollback_fn
                .get_or_insert(Vec::new())
                .push(
                    Box::new(|dest_res: &mut Resources, res: &Resources|{
                        dest_res.insert(res.get_cloned::<R>().unwrap());
                    })
            );
        }
        self
    }

    fn  override_resouce<R: Resource + Clone>(&mut self) -> &mut Self{
        {
            let mut rollback_buffer = self.resources().get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!");

            rollback_buffer
                .resource_overrides
                .get_or_insert(Vec::new())
                .push(
                Box::new(|dest_res: &mut Resources, res: &Resources|{
                    dest_res.insert(res.get_cloned::<R>().unwrap());
                })
            );
        }
        self
    }
}