use anyhow::{anyhow, Context, Result};
use log::{debug, info, warn};
use rustpython_parser::ast::{Mod, Stmt, StmtImportFrom};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::dependency_graph::{DependencyGraph, ModuleNode};
use crate::emit::CodeEmitter;
use crate::resolver::{ImportType, ModuleResolver};

/// Type alias for module processing queue
type ModuleQueue = Vec<(String, PathBuf)>;
/// Type alias for processed modules set
type ProcessedModules = HashSet<String>;

/// Parameters for discovery phase operations
struct DiscoveryParams<'a> {
  resolver: &'a mut ModuleResolver,
  modules_to_process: &'a mut ModuleQueue,
  processed_modules: &'a ProcessedModules,
  queued_modules: &'a mut HashSet<String>,
}

pub struct Bundler {
  config: Config,
}

impl Bundler {
  pub fn new(config: Config) -> Self {
    Self { config }
  }

  /// Main bundling function
  pub fn bundle(
    &mut self,
    entry_path: &Path,
    output_path: &Path,
    emit_requirements: bool,
  ) -> Result<()> {
    info!("Starting bundle process");
    debug!("Entry: {:?}, Output: {:?}", entry_path, output_path);

    // Auto-detect the entry point's directory as a source directory
    if let Some(entry_dir) = entry_path.parent() {
      // Canonicalize the path to avoid duplicates due to different lexical representations
      let entry_dir = match entry_dir.canonicalize() {
        Ok(canonical_path) => canonical_path,
        Err(_) => {
          // Fall back to the original path if canonicalization fails (e.g., path doesn't exist)
          entry_dir.to_path_buf()
        }
      };
      if !self.config.src.contains(&entry_dir) {
        debug!("Adding entry directory to src paths: {:?}", entry_dir);
        self.config.src.insert(0, entry_dir);
      }
    }

    // Initialize resolver
    let mut resolver = ModuleResolver::new(self.config.clone())?;

    // Find the entry module name
    let entry_module_name = self.find_entry_module_name(entry_path, &resolver)?;
    info!("Entry module: {}", entry_module_name);

    // Build dependency graph
    let mut graph = self.build_dependency_graph(entry_path, &entry_module_name, &mut resolver)?;

    // Filter to only modules reachable from entry
    debug!(
      "Before filtering - graph has {} modules",
      graph.get_modules().len()
    );
    graph = graph.filter_reachable_from(&entry_module_name)?;
    debug!(
      "After filtering - graph has {} modules",
      graph.get_modules().len()
    );

    // Check for cycles
    if graph.has_cycles() {
      return Err(anyhow!(
        "Circular dependencies detected in the module graph"
      ));
    }

    // Get topologically sorted modules
    let sorted_modules = graph.topological_sort()?;
    info!("Found {} modules to bundle", sorted_modules.len());
    for (i, module) in sorted_modules.iter().enumerate() {
      debug!("Module {}: {} ({:?})", i, module.name, module.path);
    }

    // Generate bundled code
    let mut emitter = CodeEmitter::new(
      resolver,
      self.config.preserve_comments,
      self.config.preserve_type_hints,
    );

    let bundled_code = emitter.emit_bundle(&sorted_modules, &entry_module_name)?;

    // Write output file
    fs::write(output_path, bundled_code)
      .with_context(|| format!("Failed to write output file: {:?}", output_path))?;

    info!("Bundle written to: {:?}", output_path);

    // Generate requirements.txt if requested
    if emit_requirements {
      self.write_requirements_file(&sorted_modules, &mut emitter, output_path)?;
    }

    Ok(())
  }

  /// Helper method to find module name in source directories
  fn find_module_in_src_dirs(&self, entry_path: &Path) -> Option<String> {
    for src_dir in &self.config.src {
      let relative_path = entry_path.strip_prefix(src_dir).ok()?;
      if let Some(module_name) = self.path_to_module_name(relative_path) {
        return Some(module_name);
      }
    }
    None
  }

  /// Find the module name for the entry script
  fn find_entry_module_name(
    &self,
    entry_path: &Path,
    _resolver: &ModuleResolver,
  ) -> Result<String> {
    // Try to find which src directory contains the entry file
    if let Some(module_name) = self.find_module_in_src_dirs(entry_path) {
      return Ok(module_name);
    }

    // If not found in src directories, use the file stem as module name
    let module_name = entry_path
      .file_stem()
      .and_then(|name| name.to_str())
      .ok_or_else(|| {
        anyhow!(
          "Cannot determine module name from entry path: {:?}",
          entry_path
        )
      })?;

    Ok(module_name.to_string())
  }

  /// Convert a relative path to a module name
  fn path_to_module_name(&self, relative_path: &Path) -> Option<String> {
    let parts: Vec<String> = relative_path
      .components()
      .map(|c| c.as_os_str().to_string_lossy().to_string())
      .collect();

    if parts.is_empty() {
      return None;
    }

    let mut module_parts = parts;
    let last_part = module_parts.last_mut()?;

    // Remove .py extension
    if last_part.ends_with(".py") {
      *last_part = last_part[..last_part.len() - 3].to_string();
    }

    // Handle __init__.py
    if last_part == "__init__" {
      module_parts.pop();
    }

    if module_parts.is_empty() {
      return None;
    }

    Some(module_parts.join("."))
  }

  /// Build the complete dependency graph starting from the entry module
  fn build_dependency_graph(
    &self,
    entry_path: &Path,
    entry_module_name: &str,
    resolver: &mut ModuleResolver,
  ) -> Result<DependencyGraph> {
    let mut graph = DependencyGraph::new();
    let mut processed_modules = ProcessedModules::new();
    let mut queued_modules = HashSet::new();
    let mut modules_to_process = ModuleQueue::new();
    modules_to_process.push((entry_module_name.to_string(), entry_path.to_path_buf()));
    queued_modules.insert(entry_module_name.to_string());

    // Store module data for phase 2
    type ModuleData = (String, PathBuf, Vec<String>);
    let mut all_modules: Vec<ModuleData> = Vec::new();

    // PHASE 1: Discover and collect all modules
    info!("Phase 1: Discovering all modules...");
    while let Some((module_name, module_path)) = modules_to_process.pop() {
      debug!("Discovering module: {} ({:?})", module_name, module_path);
      if processed_modules.contains(&module_name) {
        debug!("Module {} already discovered, skipping", module_name);
        continue;
      }

      // Parse the module and extract imports
      let imports = self.extract_imports(&module_path)?;
      debug!("Extracted imports from {}: {:?}", module_name, imports);

      // Store module data for later processing
      all_modules.push((module_name.clone(), module_path.clone(), imports.clone()));
      processed_modules.insert(module_name.clone());

      // Find and queue first-party imports for discovery
      for import in imports {
        let mut params = DiscoveryParams {
          resolver,
          modules_to_process: &mut modules_to_process,
          processed_modules: &processed_modules,
          queued_modules: &mut queued_modules,
        };
        self.process_import_for_discovery(&import, &mut params);
      }
    }

    info!("Phase 1 complete: discovered {} modules", all_modules.len());

    // PHASE 2: Add all modules to graph and create dependency edges
    info!("Phase 2: Adding modules to graph...");

    // First, add all modules to the graph
    for (module_name, module_path, imports) in &all_modules {
      let module_node = ModuleNode {
        name: module_name.clone(),
        path: module_path.clone(),
        imports: imports.clone(),
      };
      debug!("Adding module to graph: {}", module_node.name);
      graph.add_module(module_node);
    }

    info!("Added {} modules to graph", graph.get_modules().len());

    // Then, add all dependency edges
    info!("Phase 2: Creating dependency edges...");
    for (module_name, _module_path, imports) in &all_modules {
      for import in imports {
        self.process_import_for_dependency(import, module_name, (resolver, &mut graph));
      }
    }

    info!(
      "Phase 2 complete: dependency graph built with {} modules",
      graph.get_modules().len()
    );
    Ok(graph)
  }

  /// Extract import statements from a Python file using AST parsing
  /// This handles all import variations including multi-line, aliased, relative, and parenthesized imports
  pub fn extract_imports(&self, file_path: &Path) -> Result<Vec<String>> {
    let source = fs::read_to_string(file_path)
      .with_context(|| format!("Failed to read file: {:?}", file_path))?;

    let parsed = rustpython_parser::parse(
      &source,
      rustpython_parser::Mode::Module,
      file_path.to_string_lossy().as_ref(),
    )
    .with_context(|| format!("Failed to parse Python file: {:?}", file_path))?;

    let mut imports = Vec::new();

    if let Mod::Module(module) = parsed {
      for stmt in module.body.iter() {
        self.extract_imports_from_statement(stmt, &mut imports);
      }
    }

    Ok(imports)
  }

  /// Extract import module names from a single AST statement
  fn extract_imports_from_statement(&self, stmt: &Stmt, imports: &mut Vec<String>) {
    if let Stmt::Import(import_stmt) = stmt {
      for alias in &import_stmt.names {
        imports.push(alias.name.to_string());
      }
    } else if let Stmt::ImportFrom(import_from_stmt) = stmt {
      self.process_import_from_statement(import_from_stmt, imports);
    }
  }

  /// Process a "from ... import ..." statement to extract module names
  fn process_import_from_statement(
    &self,
    import_from_stmt: &StmtImportFrom,
    imports: &mut Vec<String>,
  ) {
    if let Some(ref module) = import_from_stmt.module {
      let level = import_from_stmt.level.map(|i| i.to_u32()).unwrap_or(0);
      let module_name = self.format_module_name(module, level);
      imports.push(module_name);
    } else if let Some(level_int) = import_from_stmt.level {
      let level = level_int.to_u32();
      if level > 0 {
        imports.push(".".repeat(level as usize));
      }
    }
  }

  /// Format module name based on relative import level
  fn format_module_name(&self, module: &str, level: u32) -> String {
    if level > 0 {
      if module.is_empty() {
        ".".to_string()
      } else {
        let dots = ".".repeat(level as usize);
        format!("{}{}", dots, module)
      }
    } else {
      module.to_string()
    }
  }

  /// Helper method to add module to discovery queue if not already processed or queued
  fn add_to_discovery_queue_if_new(
    &self,
    import: &str,
    import_path: PathBuf,
    discovery_params: &mut DiscoveryParams,
  ) {
    if !discovery_params.processed_modules.contains(import)
      && !discovery_params.queued_modules.contains(import)
    {
      debug!("Adding '{}' to discovery queue", import);
      discovery_params
        .modules_to_process
        .push((import.to_string(), import_path));
      discovery_params.queued_modules.insert(import.to_string());
    } else {
      debug!("Module '{}' already processed or queued, skipping", import);
    }
  }

  /// Process an import during discovery phase
  fn process_import_for_discovery(&self, import: &str, params: &mut DiscoveryParams) {
    match params.resolver.classify_import(import) {
      ImportType::FirstParty => {
        debug!("'{}' classified as FirstParty", import);
        if let Ok(Some(import_path)) = params.resolver.resolve_module_path(import) {
          debug!("Resolved '{}' to path: {:?}", import, import_path);
          self.add_to_discovery_queue_if_new(import, import_path, params);
        } else {
          warn!("Failed to resolve path for first-party module: {}", import);
        }
      }
      ImportType::ThirdParty | ImportType::StandardLibrary => {
        debug!("'{}' classified as external (preserving)", import);
      }
    }
  }

  /// Process an import during dependency graph creation phase
  fn process_import_for_dependency(
    &self,
    import: &str,
    module_name: &str,
    resolver_and_graph: (&ModuleResolver, &mut DependencyGraph),
  ) {
    let (resolver, graph) = resolver_and_graph;
    match resolver.classify_import(import) {
      ImportType::FirstParty => {
        // Add dependency edge (dependency -> dependent)
        // This means the imported module must come before the importing module
        debug!("Adding dependency edge: {} -> {}", import, module_name);
        if let Err(e) = graph.add_dependency(import, module_name) {
          debug!("Failed to add dependency edge: {}", e);
          warn!("Failed to add dependency edge: {}", e);
        } else {
          debug!(
            "Successfully added dependency edge: {} -> {}",
            import, module_name
          );
        }
      }
      ImportType::ThirdParty | ImportType::StandardLibrary => {
        // These will be preserved in the output, not inlined
      }
    }
  }

  /// Write requirements.txt file if there are dependencies
  fn write_requirements_file(
    &self,
    sorted_modules: &[&ModuleNode],
    emitter: &mut CodeEmitter,
    output_path: &Path,
  ) -> Result<()> {
    let requirements_content = emitter.generate_requirements(sorted_modules)?;
    if !requirements_content.is_empty() {
      let requirements_path = output_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("requirements.txt");

      fs::write(&requirements_path, requirements_content)
        .with_context(|| format!("Failed to write requirements file: {:?}", requirements_path))?;

      info!("Requirements written to: {:?}", requirements_path);
    } else {
      info!("No third-party dependencies found, skipping requirements.txt");
    }
    Ok(())
  }
}
