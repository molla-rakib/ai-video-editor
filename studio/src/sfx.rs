//! The built-in sound effects. They are synthesized with FFmpeg the first time
//! they're needed (original sounds, so there are no licensing questions).

use std::path::PathBuf;

use anyhow::{Context, ensure};

use crate::ffmpeg;

pub struct Sound {
    pub id: &'static str,
    pub name: &'static str,
    pub hint: &'static str,
    /// An FFmpeg lavfi source producing the sound.
    recipe: &'static str,
}

pub const SOUNDS: [Sound; 9] = [
    Sound {
        id: "whoosh",
        name: "Whoosh",
        hint: "Soft air sweep for transitions",
        recipe: "anoisesrc=d=0.9:c=pink:a=0.9,lowpass=f=2200,highpass=f=180,volume='pow(sin(PI*t/0.9),2)':eval=frame,apulsator=hz=0.6:amount=0.6,volume=9dB,alimiter=limit=0.8,aformat=channel_layouts=stereo",
    },
    Sound {
        id: "swoosh",
        name: "Swoosh",
        hint: "Quick, bright swipe",
        recipe: "anoisesrc=d=0.45:c=white:a=0.7,bandpass=f=3200:width_type=o:w=2,volume='pow(sin(PI*t/0.45),3)':eval=frame,aformat=channel_layouts=stereo",
    },
    Sound {
        id: "pop",
        name: "Pop",
        hint: "Bubbly pop for things appearing",
        recipe: "aevalsrc='0.8*sin(2*PI*(380*t+1400*(1-exp(-30*t))/30))*exp(-22*t)':d=0.25:s=48000,aformat=channel_layouts=stereo",
    },
    Sound {
        id: "click",
        name: "Click",
        hint: "Crisp UI click",
        recipe: "aevalsrc='(random(0)*2-1)*exp(-500*t)*0.9+0.5*sin(2*PI*2400*t)*exp(-260*t)':d=0.06:s=48000,highpass=f=900,volume=-4dB,aformat=channel_layouts=stereo",
    },
    Sound {
        id: "ding",
        name: "Ding",
        hint: "Bright bell for highlights",
        recipe: "aevalsrc='0.45*(sin(2*PI*1318.5*t)+0.45*sin(2*PI*2637*t)+0.2*sin(2*PI*3955*t))*exp(-3.5*t)':d=1.4:s=48000,aformat=channel_layouts=stereo",
    },
    Sound {
        id: "riser",
        name: "Riser",
        hint: "Builds tension before a reveal",
        recipe: "aevalsrc='(0.35*sin(2*PI*(180*t+140*t*t))+0.25*(random(0)*2-1))*pow(t/2.4,2)':d=2.4:s=48000,lowpass=f=5000,aformat=channel_layouts=stereo",
    },
    Sound {
        id: "impact",
        name: "Impact",
        hint: "Deep hit for big moments",
        recipe: "aevalsrc='0.9*sin(2*PI*(48*t+90*(1-exp(-9*t))/9))*exp(-2.8*t)+0.3*(random(0)*2-1)*exp(-30*t)':d=1.6:s=48000,lowpass=f=900,aformat=channel_layouts=stereo",
    },
    Sound {
        id: "typing",
        name: "Typing",
        hint: "Keyboard clicks for screen demos",
        recipe: "aevalsrc='(random(0)*2-1)*exp(-320*mod(t+0.03*sin(7*t),0.115))*0.5':d=1.6:s=48000,highpass=f=1500,volume=9dB,alimiter=limit=0.8,aformat=channel_layouts=stereo",
    },
    Sound {
        id: "chime",
        name: "Success chime",
        hint: "Two-note confirmation",
        recipe: "aevalsrc='0.4*sin(2*PI*880*t)*exp(-6*t)*lt(t,0.35)+0.4*sin(2*PI*1318.5*(t-0.12))*exp(-4*(t-0.12))*gte(t,0.12)':d=1.2:s=48000,aformat=channel_layouts=stereo",
    },
];

fn dir() -> PathBuf {
    crate::settings::config_dir().join("sounds")
}

/// Path of the sound's file, synthesizing it first if needed.
pub fn path(id: &str) -> anyhow::Result<PathBuf> {
    let sound = SOUNDS.iter().find(|s| s.id == id).context("Unknown sound")?;
    let path = dir().join(format!("{}.wav", sound.id));
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(dir())?;
    let tmp = path.with_extension("tmp.wav");
    let ok = ffmpeg::ffmpeg()?.args(["-f", "lavfi", "-i", sound.recipe, "-ar", "48000", "-c:a", "pcm_s16le", "-y"]).arg(&tmp).status()?.success();
    ensure!(ok, "Couldn't create the {} sound", sound.name);
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_recipe_synthesizes_audio() {
        for s in &SOUNDS {
            let out = ffmpeg::ffmpeg().unwrap().args(["-f", "lavfi", "-i", s.recipe, "-f", "null", "-"]).output().unwrap();
            assert!(out.status.success(), "{}: {}", s.id, String::from_utf8_lossy(&out.stderr));
        }
    }
}
