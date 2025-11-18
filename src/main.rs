use std::io::{self, Write};
use std::time::Duration;
use std::f32::consts::PI;
use std::fs;
use std::sync::{Arc, Mutex};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use dialoguer::{Select, Input, Confirm, theme::ColorfulTheme};
use serde::{Deserialize, Serialize};

//
// =========================
//   V O I C E
// =========================
//

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Waveform { Sine, Saw, Square, Triangle }

#[derive(Clone, Debug)]
pub struct Voice {
    phase: f32,
    frequency: f32,
    waveform: Waveform,
    amp: f32,
    // simple ADSR
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    env_phase: f32,
}

impl Voice {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            frequency: 440.0,
            waveform: Waveform::Saw,
            amp: 0.15,
            attack: 0.01,
            decay: 0.1,
            sustain: 0.3,
            release: 0.1,
            env_phase: 0.0,
        }
    }

    pub fn set_frequency(&mut self, freq: f32) { self.frequency = freq; }

    pub fn process(&mut self, sample_rate: f32) -> f32 {
        let sample = match self.waveform {
            Waveform::Saw => 2.0 * (self.phase - 0.5),
            Waveform::Sine => (2.0 * PI * self.phase).sin(),
            Waveform::Square => if self.phase < 0.5 { 1.0 } else { -1.0 },
            Waveform::Triangle => 1.0 - (4.0 * (self.phase - 0.25)).abs(),
        };

        self.phase += self.frequency / sample_rate;
        if self.phase >= 1.0 { self.phase -= 1.0; }

        // simple envelope
        let env = if self.env_phase < self.attack {
            self.env_phase / self.attack
        } else if self.env_phase < self.attack + self.decay {
            1.0 - ((self.env_phase - self.attack)/self.decay)*(1.0 - self.sustain)
        } else {
            self.sustain
        };

        self.env_phase += 1.0 / sample_rate;

        sample * self.amp * env
    }

    pub fn reset_env(&mut self) { self.env_phase = 0.0; }
}

//
// =========================
//   S C A L E + UTILS
// =========================
//

fn midi_to_freq(n: i32) -> f32 { 440.0 * 2f32.powf((n as f32 - 69.0)/12.0) }

fn minor_scale(root: &str) -> Vec<i32> {
    let r = note_to_semitone(root);
    vec![0,2,3,5,7,8,10].iter().map(|x| x+r).collect()
}

fn note_to_semitone(name: &str) -> i32 {
    match name.to_lowercase().as_str() {
        "c"=>0,"c#"|"db"=>1,"d"=>2,"d#"|"eb"=>3,"e"=>4,"f"=>5,"f#"|"gb"=>6,
        "g"=>7,"g#"|"ab"=>8,"a"=>9,"a#"|"bb"=>10,"b"=>11,_=>0
    }
}

//
// =========================
//   T R A C K
// =========================
//

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub pattern: Vec<i32>,
    pub octave: i32,
    pub transpose: i32,
    pub waveform: Waveform,
    pub voice_spread: i32,
}

impl Track {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            pattern: vec![0],
            octave: 3,
            transpose: 0,
            waveform: Waveform::Saw,
            voice_spread: 7,
        }
    }
}

//
// =========================
//   S E Q U E N C E R
// =========================
//

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectData {
    pub tracks: Vec<Track>,
    pub scale: Vec<i32>,
    pub bpm: f32,
}

#[derive(Clone, Debug)]
pub struct Sequencer {
    pub tracks: Vec<Track>,
    pub scale: Vec<i32>,
    pub voices: Vec<Vec<Voice>>,

    pub sample_rate: f32,
    pub step: usize,
    pub samples_per_step: usize,
    pub sample_counter: usize,
}

impl Sequencer {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            tracks: vec![Track::new("Main")],
            scale: minor_scale("g"),
            voices: vec![vec![Voice::new(); 3]],
            sample_rate,
            step: 0,
            samples_per_step: (sample_rate/4.0) as usize,
            sample_counter: 0,
        }
    }

    pub fn from_project(project: ProjectData, sample_rate: f32) -> Self {
        let num_tracks = project.tracks.len();
        let mut voices = Vec::new();
        for _ in 0..num_tracks {
            voices.push(vec![Voice::new(); 3]);
        }
        
        Self {
            tracks: project.tracks,
            scale: project.scale,
            voices,
            sample_rate,
            step: 0,
            samples_per_step: (sample_rate * 60.0 / project.bpm / 4.0) as usize,
            sample_counter: 0,
        }
    }

    pub fn add_track(&mut self, track: Track) {
        self.tracks.push(track);
        self.voices.push(vec![Voice::new(); 3]);
    }

    pub fn process(&mut self) -> f32 {
        self.sample_counter += 1;
        if self.sample_counter >= self.samples_per_step {
            self.sample_counter = 0;
            self.step = (self.step + 1) % self.get_max_pattern_len();
            self.trigger_step();
        }

        // mix all tracks
        let mut sum = 0.0;
        let mut voice_count = 0;
        for voices in &mut self.voices {
            for v in voices {
                sum += v.process(self.sample_rate);
                voice_count += 1;
            }
        }
        if voice_count > 0 {
            sum / voice_count as f32
        } else {
            0.0
        }
    }

    fn get_max_pattern_len(&self) -> usize {
        self.tracks.iter().map(|t| t.pattern.len()).max().unwrap_or(1)
    }

    fn trigger_step(&mut self) {
        for (track_idx, track) in self.tracks.iter().enumerate() {
            if track.pattern.is_empty() { continue; }
            
            let note = track.pattern[self.step % track.pattern.len()];
            if note < 0 { continue; } // rest
            
            let scale_note = self.scale[(note as usize) % self.scale.len()];
            let midi_base = scale_note + track.transpose + track.octave*12;

            if track_idx < self.voices.len() {
                for (i, v) in self.voices[track_idx].iter_mut().enumerate() {
                    let freq = midi_to_freq(midi_base + i as i32 * track.voice_spread);
                    v.set_frequency(freq);
                    v.waveform = track.waveform;
                    v.reset_env();
                }
            }
        }
    }

    pub fn to_project(&self, bpm: f32) -> ProjectData {
        ProjectData {
            tracks: self.tracks.clone(),
            scale: self.scale.clone(),
            bpm,
        }
    }
}

//
// =========================
//   A U D I O
// =========================
//

fn play_audio(seq: Arc<Mutex<Sequencer>>) {
    let host = cpal::default_host();
    let device = host.default_output_device().expect("no output device");
    let config = device.default_output_config().unwrap();
    let err_fn = |err| eprintln!("stream error: {err}");

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let cfg = config.clone().into();
            let seq = seq.clone();
            device.build_output_stream(&cfg, move |data: &mut [f32], _| {
                if let Ok(mut s) = seq.lock() {
                    for sample in data { 
                        *sample = s.process(); 
                    }
                }
            }, err_fn, None).unwrap()
        }
        cpal::SampleFormat::I16 => {
            let cfg = config.clone().into();
            let seq = seq.clone();
            device.build_output_stream(&cfg, move |data: &mut [i16], _| {
                if let Ok(mut s) = seq.lock() {
                    for sample in data { 
                        *sample = (s.process()*i16::MAX as f32) as i16; 
                    }
                }
            }, err_fn, None).unwrap()
        }
        cpal::SampleFormat::U16 => {
            let cfg = config.clone().into();
            let seq = seq.clone();
            device.build_output_stream(&cfg, move |data: &mut [u16], _| {
                if let Ok(mut s) = seq.lock() {
                    for sample in data {
                        let v = (s.process()*0.5+0.5).clamp(0.0,1.0);
                        *sample = (v*u16::MAX as f32) as u16;
                    }
                }
            }, err_fn, None).unwrap()
        }
        _ => panic!("Unsupported sample format"),
    };

    stream.play().unwrap();
    // Silently run - don't print to console
    loop { std::thread::sleep(Duration::from_secs(1)); }
}

//
// =========================
//   P A R S E R
// =========================
//

fn parse_track_line(line: &str) -> Option<Track> {
    let mut track = Track::new("Untitled");
    
    // Parse pattern: n"0 3 5 7"
    if let Some(start) = line.find("n\"") {
        if let Some(end_pos) = line[start+2..].find("\"") {
            let inside = &line[start+2..start+2+end_pos];
            track.pattern = inside.split_whitespace()
                .filter_map(|x| x.parse::<i32>().ok())
                .collect();
        }
    }
    
    // Parse octave: .o(3)
    if line.contains(".o(") {
        if let Some(open) = line.find(".o(") {
            if let Some(close) = line[open..].find(")") {
                let val = &line[open+3..open+close];
                if let Ok(oct) = val.parse() { track.octave = oct; }
            }
        }
    }
    
    // Parse transpose: .trans(5)
    if line.contains(".trans(") {
        if let Some(open) = line.find(".trans(") {
            if let Some(close) = line[open..].find(")") {
                let val = &line[open+7..open+close];
                if let Ok(tr) = val.parse() { track.transpose = tr; }
            }
        }
    }
    
    // Parse waveform: .s("saw")
    if line.contains(".s(") {
        if line.contains("saw") { track.waveform = Waveform::Saw; }
        else if line.contains("sine") { track.waveform = Waveform::Sine; }
        else if line.contains("square") { track.waveform = Waveform::Square; }
        else if line.contains("triangle") { track.waveform = Waveform::Triangle; }
    }
    
    Some(track)
}

//
// =========================
//   I N T E R A C T I V E
// =========================
//

fn create_track_interactive(theme: &ColorfulTheme) -> Option<Track> {
    println!("\n=== Create New Track ===");
    
    let name: String = Input::with_theme(theme)
        .with_prompt("Track name")
        .default("Synth".to_string())
        .interact_text()
        .ok()?;
    
    let pattern_str: String = Input::with_theme(theme)
        .with_prompt("Pattern (space-separated notes, -1 for rest)")
        .default("0 3 5 7 0 5 3 0".to_string())
        .interact_text()
        .ok()?;
    
    let pattern: Vec<i32> = pattern_str.split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect();
    
    let octave: i32 = Input::with_theme(theme)
        .with_prompt("Octave")
        .default(3)
        .interact_text()
        .ok()?;
    
    let transpose: i32 = Input::with_theme(theme)
        .with_prompt("Transpose (semitones)")
        .default(0)
        .interact_text()
        .ok()?;
    
    let waveforms = vec!["Saw", "Sine", "Square", "Triangle"];
    let wave_idx = Select::with_theme(theme)
        .with_prompt("Waveform")
        .default(0)
        .items(&waveforms)
        .interact()
        .ok()?;
    
    let waveform = match wave_idx {
        0 => Waveform::Saw,
        1 => Waveform::Sine,
        2 => Waveform::Square,
        3 => Waveform::Triangle,
        _ => Waveform::Saw,
    };
    
    Some(Track {
        name,
        pattern,
        octave,
        transpose,
        waveform,
        voice_spread: 7,
    })
}

fn save_project(seq: &Arc<Mutex<Sequencer>>, theme: &ColorfulTheme) {
    let filename: String = Input::with_theme(theme)
        .with_prompt("Save as")
        .default("track.json".to_string())
        .interact_text()
        .unwrap();
    
    let bpm: f32 = Input::with_theme(theme)
        .with_prompt("BPM")
        .default(120.0)
        .interact_text()
        .unwrap();
    
    if let Ok(s) = seq.lock() {
        let project = s.to_project(bpm);
        let json = serde_json::to_string_pretty(&project).unwrap();
        fs::write(&filename, json).unwrap();
        println!("✓ Saved to {}", filename);
    }
}

fn load_project(theme: &ColorfulTheme) -> Option<ProjectData> {
    let filename: String = Input::with_theme(theme)
        .with_prompt("Load file")
        .default("track.json".to_string())
        .interact_text()
        .ok()?;
    
    let json = fs::read_to_string(&filename).ok()?;
    let project: ProjectData = serde_json::from_str(&json).ok()?;
    println!("✓ Loaded from {}", filename);
    Some(project)
}

fn repl_mode(seq: &Arc<Mutex<Sequencer>>) {
    println!("\n╔═══════════════════════════════════════════════════════════╗");
    println!("║          R E P L   M O D E                                ║");
    println!("╚═══════════════════════════════════════════════════════════╝");
    println!("Build your track line by line. Each line creates/modifies a track.");
    println!("\nCommands:");
    println!("  [name] n\"0 3 5 7\" .o(3) .s(\"saw\") .trans(0)");
    println!("  list              - show all tracks");
    println!("  clear             - remove all tracks");
    println!("  delete <name>     - remove a specific track");
    println!("  exit              - return to main menu");
    println!("\nExample:");
    println!("  bass n\"0 0 -1 0\" .o(2) .s(\"sine\")");
    println!("  lead n\"0 3 5 7 5 3\" .o(4) .s(\"saw\") .trans(5)");
    println!("  pad n\"0 2 4\" .o(3) .s(\"triangle\")\n");

    loop {
        print!("repl> ");
        io::stdout().flush().unwrap();
        
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let input = input.trim();
        
        if input.is_empty() { continue; }
        
        match input {
            "exit" => {
                println!("Exiting REPL mode...");
                break;
            }
            "list" => {
                if let Ok(s) = seq.lock() {
                    if s.tracks.is_empty() {
                        println!("  (no tracks)");
                    } else {
                        println!("\n=== Current Tracks ===");
                        for (idx, track) in s.tracks.iter().enumerate() {
                            println!("  {}. {} - Pattern: {:?}, O:{}, T:{}, W:{:?}", 
                                idx + 1, track.name, track.pattern, 
                                track.octave, track.transpose, track.waveform);
                        }
                    }
                }
            }
            "clear" => {
                if let Ok(mut s) = seq.lock() {
                    s.tracks.clear();
                    s.voices.clear();
                    println!("✓ All tracks cleared");
                }
            }
            _ if input.starts_with("delete ") => {
                let name = input.strip_prefix("delete ").unwrap().trim();
                if let Ok(mut s) = seq.lock() {
                    if let Some(pos) = s.tracks.iter().position(|t| t.name == name) {
                        s.tracks.remove(pos);
                        s.voices.remove(pos);
                        println!("✓ Deleted track '{}'", name);
                    } else {
                        println!("✗ Track '{}' not found", name);
                    }
                }
            }
            _ => {
                // Parse track line
                let parts: Vec<&str> = input.splitn(2, ' ').collect();
                if parts.len() < 2 {
                    println!("✗ Format: <name> n\"pattern\" .o(octave) .s(\"wave\")");
                    continue;
                }
                
                let name = parts[0];
                let rest = parts[1];
                
                if let Some(mut track) = parse_track_line(rest) {
                    track.name = name.to_string();
                    
                    if let Ok(mut s) = seq.lock() {
                        // Check if track with same name exists
                        if let Some(existing) = s.tracks.iter_mut().find(|t| t.name == name) {
                            *existing = track.clone();
                            println!("✓ Updated track '{}'", name);
                        } else {
                            s.add_track(track);
                            println!("✓ Added track '{}' (playing now!)", name);
                        }
                    }
                } else {
                    println!("✗ Failed to parse track");
                }
            }
        }
    }
}

//
// =========================
//   M A I N
// =========================
//

fn main() {
    let theme = ColorfulTheme::default();
    
    println!("╔═══════════════════════════════╗");
    println!("║   V I B E Z  T R A N C E      ║");
    println!("╚═══════════════════════════════╝\n");
    
    let options = vec![
        "REPL Mode - Build tracks as you go",
        "Create new track (interactive)",
        "Import track from file",
        "Start with example",
    ];
    
    let choice = Select::with_theme(&theme)
        .with_prompt("What would you like to do?")
        .default(0)
        .items(&options)
        .interact()
        .unwrap();
    
    let seq = match choice {
        0 => {
            // REPL Mode - start with empty sequencer
            let mut s = Sequencer::new(44100.0);
            s.tracks.clear();
            s.voices.clear();
            Arc::new(Mutex::new(s))
        }
        1 => {
            // Create new
            let mut s = Sequencer::new(44100.0);
            s.tracks.clear();
            s.voices.clear();
            
            loop {
                if let Some(track) = create_track_interactive(&theme) {
                    s.add_track(track);
                }
                
                if !Confirm::with_theme(&theme)
                    .with_prompt("Add another track?")
                    .default(false)
                    .interact()
                    .unwrap()
                {
                    break;
                }
            }
            Arc::new(Mutex::new(s))
        }
        2 => {
            // Import
            if let Some(project) = load_project(&theme) {
                Arc::new(Mutex::new(Sequencer::from_project(project, 44100.0)))
            } else {
                println!("Failed to load. Using default.");
                Arc::new(Mutex::new(Sequencer::new(44100.0)))
            }
        }
        3 => {
            // Example
            let mut s = Sequencer::new(44100.0);
            s.tracks.clear();
            s.voices.clear();
            
            let mut bass = Track::new("Bass");
            bass.pattern = vec![0, 0, -1, 0, 3, 3, -1, 3];
            bass.octave = 2;
            bass.waveform = Waveform::Sine;
            s.add_track(bass);
            
            let mut lead = Track::new("Lead");
            lead.pattern = vec![0, 3, 5, 7, 5, 3, 0, -1];
            lead.octave = 4;
            lead.waveform = Waveform::Saw;
            s.add_track(lead);
            
            println!("✓ Loaded example with bass + lead");
            Arc::new(Mutex::new(s))
        }
        _ => Arc::new(Mutex::new(Sequencer::new(44100.0))),
    };
    
    // Display tracks
    if let Ok(s) = seq.lock() {
        if !s.tracks.is_empty() {
            println!("\n=== Loaded Tracks ===");
            for track in &s.tracks {
                println!("  • {} (O:{} T:{} W:{:?})", 
                    track.name, track.octave, track.transpose, track.waveform);
            }
        }
    }
    
    // Start audio
    let seq_audio = seq.clone();
    std::thread::spawn(move || { play_audio(seq_audio); });
    
    // Give audio thread time to start
    std::thread::sleep(Duration::from_millis(100));
    println!("🎶 Audio running...\n");
    
    // If user chose REPL mode, go straight into it
    if choice == 0 {
        repl_mode(&seq);
    }
    
    // Menu loop
    loop {
        println!("\n=== Menu ===");
        let menu_options = vec![
            "REPL Mode (build as you go)",
            "Add track (interactive)",
            "Save project",
            "Quit",
        ];
        
        let menu_choice = Select::with_theme(&theme)
            .with_prompt("Choose action")
            .default(0)
            .items(&menu_options)
            .interact()
            .unwrap();
        
        match menu_choice {
            0 => {
                repl_mode(&seq);
            }
            1 => {
                if let Some(track) = create_track_interactive(&theme) {
                    if let Ok(mut s) = seq.lock() {
                        s.add_track(track);
                        println!("✓ Track added (playing now!)");
                    }
                }
            }
            2 => {
                save_project(&seq, &theme);
            }
            3 => {
                println!("Goodbye! 🎵");
                break;
            }
            _ => {}
        }
    }
}
