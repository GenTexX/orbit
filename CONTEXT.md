# Orbit

An educational 2D game engine in Rust: a core engine library, an editor application with a custom retained GUI, and a runtime that plays packaged games. Games are scripted in a custom language compiled to WASM. The project as a whole is known publicly as **Orbit**; internally each part carries its own celestial name - Photon (renderer), Helios (engine), Voyager (runtime), Atlas (editor), Aurora (GUI framework), Comet (language). These are working crate names; what the public eventually calls each part is open.

## Language

**Aurora**:
The custom retained GUI framework the Editor is built with. Engine-independent and reusable outside Orbit; consumes input, produces draw lists.
_Avoid_: the GUI, orbit-gui

**Widget**:
A node in Aurora's UI tree - panel, label, button, checkbox, or text input - stored in the widget arena, laid out by taffy, and referenced by a typed handle.
_Avoid_: control, element

**Draw list**:
The per-frame list of high-level drawing commands (filled rects, text runs, clips) Aurora produces and aurora-wgpu renders. Aurora's boundary with the GPU.
_Avoid_: render list, command buffer

**Comet**:
The scripting language: statically typed, garbage-collected, compiled to WASM. Script source files use the .cmt extension.
_Avoid_: the scripting language (in docs), orbit-script

**Photon**:
The 2D drawing crate - sprites, shapes, text, render targets - and the only code that touches wgpu.
_Avoid_: the renderer (as a proper name), orbit-renderer

**Helios**:
The crate implementing the Engine.
_Avoid_: orbit-engine

**Voyager**:
The crate implementing the Runtime.
_Avoid_: orbit-runtime

**Atlas**:
The crate implementing the Editor.
_Avoid_: orbit-editor

**Engine**:
The core library of reusable game technology - rendering, input, audio, physics, scene management, script host. Has no main function and opens no windows; it is embedded by the Runtime and the Editor.
_Avoid_: framework, core (as a standalone noun)

**Runtime**:
The component that loads a packaged game and plays it using the Engine. It is a library with a thin shipping binary; the Editor links the same library to power the Play button.
_Avoid_: player, game host

**Editor**:
The application where games are authored - scene editing, inspector, code editing, asset management. Embeds the Runtime in-process for Play.
_Avoid_: IDE, tool (unqualified)

**Viewport**:
The editor panel that displays a running or edited scene, rendered by the Engine into a texture.
_Avoid_: preview, game view

**Scene**:
A tree of Nodes that can be loaded, played, or instanced inside another Scene.
_Avoid_: level, map, world

**Instance**:
An occurrence of a Scene embedded as a subtree of another Scene. May override component property values on its nodes, but never structure.
_Avoid_: prefab, copy

**Node**:
A named element in a Scene's tree with a transform and children - and nothing else. All capability comes from attached Components.
_Avoid_: entity, game object, actor

**Component**:
A unit of capability attached to a Node - sprite, camera, collider, script. What the inspector edits and what serialization walks.
_Avoid_: behavior, module, trait (in domain conversation)

**Script**:
A Component whose behavior is defined by user code in the scripting language. Attaching a Script is the only way user code enters a Scene.
_Avoid_: behavior script, custom component

**Project**:
The authoring artifact: a directory containing a manifest, scene files, script sources, and assets - what the Editor opens and what lives in version control.
_Avoid_: project file (singular), workspace

**Game Package**:
The shipping artifact: a single binary file produced by building a Project, containing everything the Runtime needs to play the game.
_Avoid_: build, bundle, pck
