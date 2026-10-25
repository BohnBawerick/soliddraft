//! Display levels for the open part: which of its three meshes the viewport shows, and
//! the tessellation that makes them off the interface thread.
//!
//! A rebuild tessellates the shown level at once, so the viewport never waits for a
//! worker to show the new shape. The other two levels follow on the worker. Each frame
//! [`DisplayQuality::update`] picks the level from how many pixels a millimetre covers,
//! with the hysteresis of `soliddraft_tessellation::select`, and says when the shown
//! mesh should change. A quality change asks the worker for all three levels and keeps
//! the old mesh on screen until the new one arrives.
//!
//! The meshes of the last few bodies shown stay while the part keeps those bodies, so
//! undo, redo and a value put back show their body's mesh again without tessellating.

use std::collections::VecDeque;
use std::sync::Arc;

use soliddraft_kernel_api::{BodyId, Kernel, Mesh, TessellationParams, Topology, Vec3};
use soliddraft_tessellation::{
    Choice, DeviceBudget, DisplayCache, Level, Levels, ParamsKey, Projection, Quality, Tessellator, Thresholds,
};

/// How many bodies shown before the current one keep their meshes.
pub(crate) const RECENT: usize = 4;

/// The open part's display levels.
pub struct DisplayQuality {
    thresholds: Thresholds,
    choice: Choice,
    levels: Levels,
    cache: DisplayCache<BodyId, Mesh>,
    worker: Tessellator<BodyId>,
    /// The displayed body, its bounding sphere and the level its mesh was made at.
    body: Option<BodyId>,
    center: Vec3,
    radius: f64,
    /// The level of the shown mesh and the parameters it was made with.
    shown: Option<(Level, ParamsKey)>,
    /// The levels the worker's latest request for the body asked for, in order, with the
    /// parameters they were asked at: the levels may change before the answer arrives.
    asked: Vec<(Level, ParamsKey)>,
    /// Bodies shown before this one whose meshes are kept, latest last.
    recent: VecDeque<BodyId>,
}

impl DisplayQuality {
    /// Display levels on `kernel`, nothing shown yet.
    pub fn new(kernel: Arc<dyn Kernel>) -> DisplayQuality {
        DisplayQuality {
            thresholds: Thresholds::default(),
            choice: Choice::default(),
            levels: Levels::new(&Quality::Auto, 0.0),
            cache: DisplayCache::new(DeviceBudget::default()),
            worker: Tessellator::new(kernel),
            body: None,
            center: Vec3::new(0.0, 0.0, 0.0),
            radius: 0.0,
            shown: None,
            asked: Vec::new(),
            recent: VecDeque::new(),
        }
    }

    pub(crate) fn fork(&self, kernel: Arc<dyn Kernel>) -> Self {
        Self {
            thresholds: self.thresholds,
            choice: self.choice,
            levels: self.levels,
            cache: self.cache.clone(),
            worker: Tessellator::new(kernel),
            body: self.body,
            center: self.center,
            radius: self.radius,
            shown: self.shown,
            asked: Vec::new(),
            recent: self.recent.clone(),
        }
    }

    /// The level shown, if a mesh is.
    pub fn shown(&self) -> Option<Level> {
        self.shown.map(|(level, _)| level)
    }

    /// The level the camera asks for.
    pub fn wanted(&self) -> Level {
        self.choice.level
    }

    /// The current levels' parameters.
    pub fn levels(&self) -> &Levels {
        &self.levels
    }

    /// True while the worker owes this body a level.
    pub fn pending(&self) -> bool {
        self.worker.pending() > 0
    }

    /// A new body to show after a rebuild: the parameters to tessellate it with at once.
    /// The previous body joins the recent ones, and the oldest recent body's meshes are
    /// dropped.
    pub fn retarget(&mut self, body: BodyId, topology: &Topology, quality: &Quality) -> TessellationParams {
        if let Some(old) = self.body.replace(body)
            && old != body
        {
            self.worker.cancel(&old);
            self.remember(old);
        }
        self.recent.retain(|b| *b != body);
        self.center = topology.bbox.center();
        self.radius = topology.bbox.diagonal() / 2.0;
        self.levels = Levels::new(quality, topology.bbox.diagonal());
        self.shown = None;
        *self.levels.params(self.choice.level)
    }

    /// The mesh of the shown level when the cache still holds it from an earlier showing
    /// of `body`, the body [`retarget`](Self::retarget) just set. Shows it, and asks the
    /// worker for any level the cache lacks.
    pub fn reuse(&mut self, body: BodyId) -> Option<Arc<Mesh>> {
        let level = self.choice.level;
        let mesh = self.cache.get(&body, self.levels.key(level))?;
        self.shown = Some((level, self.levels.key(level)));
        self.request_missing(body);
        Some(mesh)
    }

    /// Drops the meshes of recent bodies `keep` refuses, such as bodies the part released.
    pub fn retain(&mut self, keep: impl Fn(BodyId) -> bool) {
        let (kept, gone): (VecDeque<BodyId>, VecDeque<BodyId>) = self.recent.drain(..).partition(|b| keep(*b));
        self.recent = kept;
        for b in gone {
            self.cache.remove(&b);
        }
    }

    /// Records the mesh made at once for the shown level, and asks the worker for the
    /// others.
    pub fn made(&mut self, body: BodyId, mesh: Mesh) {
        let level = self.choice.level;
        self.cache.insert(body, level, self.levels.key(level), mesh);
        self.shown = Some((level, self.levels.key(level)));
        self.request_missing(body);
    }

    /// Clears what is shown, as when the part has no body. The body shown joins the
    /// recent ones.
    pub fn clear(&mut self) {
        if let Some(old) = self.body.take() {
            self.worker.cancel(&old);
            self.remember(old);
        }
        self.shown = None;
    }

    fn remember(&mut self, body: BodyId) {
        self.recent.retain(|b| *b != body);
        self.recent.push_back(body);
        while self.recent.len() > RECENT {
            if let Some(oldest) = self.recent.pop_front() {
                self.cache.remove(&oldest);
            }
        }
    }

    /// A new quality for the same body: every level comes from the worker, and the
    /// shown mesh stays until its replacement arrives.
    pub fn set_quality(&mut self, quality: &Quality) {
        let Some(body) = self.body else { return };
        self.levels = Levels::new(quality, self.radius * 2.0);
        self.request_missing(body);
    }

    fn request_missing(&mut self, body: BodyId) {
        let missing = self.cache.missing(&body, &self.levels);
        if missing.is_empty() {
            return;
        }
        let params: Vec<TessellationParams> = missing.iter().map(|l| *self.levels.params(*l)).collect();
        // The worker answers in level order; remember which levels were asked for, and
        // at which parameters, since the quality can change before the answer.
        self.asked = missing.into_iter().map(|l| (l, self.levels.key(l))).collect();
        self.worker.request(body, body, &params);
    }

    /// Takes the worker's answers and moves the level choice on by one frame. Returns
    /// the mesh to show and its level when the shown mesh should change.
    pub fn update(&mut self, projection: &Projection, dt_s: f32) -> Option<(Level, Arc<Mesh>)> {
        let body = self.body?;
        for done in self.worker.poll() {
            if done.key != body {
                continue;
            }
            match done.result {
                Ok(meshes) => {
                    for ((level, key), mesh) in self.asked.clone().into_iter().zip(meshes) {
                        self.cache.insert(body, level, key, mesh);
                    }
                }
                Err(e) => log::warn!("tessellating body {body:?}: {e}"),
            }
        }
        self.cache.tick();
        let pixels_per_mm = projection.pixels_per_mm(self.center, self.radius);
        let ceiling = self.cache.ceiling();
        self.thresholds.update(&mut self.choice, &self.levels, pixels_per_mm, dt_s, ceiling);
        let wanted = (self.choice.level, self.levels.key(self.choice.level));
        if self.shown == Some(wanted) {
            return None;
        }
        let mesh = self.cache.get(&body, wanted.1)?;
        self.shown = Some(wanted);
        self.cache.trim();
        Some((wanted.0, mesh))
    }
}
