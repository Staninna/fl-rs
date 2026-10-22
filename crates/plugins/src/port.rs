//! How closely each built plugin follows FL's own. `tools/progress.py`
//! reads [`PORTS`] for the README's overview and checks it against
//! `docs/plugins.md`, so keep one entry a line. `docs/plugins.md` names
//! what is approximated or left out in each.

/// How close a plugin comes to FL's, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Port {
    /// Matches FL's output sample for sample in a null test.
    Identical,
    /// Every part read from the binary and rebuilt, nothing known to be
    /// approximated, not yet checked against FL's output.
    Exact,
    /// Exact but for named parts that are approximated or left out.
    Gaps,
    /// The sound is rebuilt approximately.
    Approximate,
}

/// Every plugin the crate hosts, by its event 201 name, but the test sine.
pub const PORTS: &[(&str, Port)] = &[
    (crate::autogun::NAME, Port::Approximate),
    (crate::balance::NAME, Port::Exact),
    (crate::bass_boost::NAME, Port::Exact),
    (crate::center::NAME, Port::Exact),
    (crate::mute_2::NAME, Port::Exact),
    (crate::phase_inverter::NAME, Port::Exact),
    (crate::bassdrum::NAME, Port::Exact),
    (crate::beepmap::NAME, Port::Gaps),
    (crate::blood_overdrive::NAME, Port::Exact),
    (crate::boobass::NAME, Port::Gaps),
    (crate::chorus::NAME, Port::Exact),
    (crate::compressor::NAME, Port::Exact),
    (crate::control_surface::NAME, Port::Gaps),
    (crate::convolver::NAME, Port::Gaps),
    (crate::delay::NAME, Port::Exact),
    (crate::delay_2::NAME, Port::Exact),
    (crate::delay_3::NAME, Port::Exact),
    (crate::db_meter::NAME, Port::Exact),
    (crate::delay_bank::NAME, Port::Gaps),
    (crate::directwave::NAME, Port::Gaps),
    (crate::drumaxx::NAME, Port::Gaps),
    (crate::drumpad::NAME, Port::Gaps),
    (crate::drumsynth_live::NAME, Port::Gaps),
    (crate::dx10::NAME, Port::Gaps),
    (crate::edison::NAME, Port::Gaps),
    (crate::effector::NAME, Port::Gaps),
    (crate::emphasis::NAME, Port::Gaps),
    (crate::emphasizer::NAME, Port::Gaps),
    (crate::envelope_controller::NAME, Port::Gaps),
    (crate::equo::NAME, Port::Gaps),
    (crate::fast_dist::NAME, Port::Exact),
    (crate::fast_lp::NAME, Port::Exact),
    (crate::filter::NAME, Port::Exact),
    (crate::fl_keys::NAME, Port::Gaps),
    (crate::fl_slayer::NAME, Port::Gaps),
    (crate::flanger::NAME, Port::Exact),
    (crate::flangus::NAME, Port::Exact),
    (crate::formula_controller::NAME, Port::Gaps),
    (crate::fpc::NAME, Port::Gaps),
    (crate::free_filter::NAME, Port::Exact),
    (crate::frequency_shifter::NAME, Port::Gaps),
    (crate::frequency_splitter::NAME, Port::Gaps),
    (crate::fruit_kick::NAME, Port::Gaps),
    (crate::granulizer::NAME, Port::Gaps),
    (crate::fruity_slicer::NAME, Port::Gaps),
    (crate::fruity_slicer_2::NAME, Port::Gaps),
    (crate::gms::NAME, Port::Gaps),
    (crate::gross_beat::NAME, Port::Gaps),
    (crate::harmless::NAME, Port::Gaps),
    (crate::harmor::NAME, Port::Approximate),
    (crate::html_notebook::NAME, Port::Exact),
    (crate::hyper_chorus::NAME, Port::Gaps),
    (crate::kepler::NAME, Port::Gaps),
    (crate::kepler_exo::NAME, Port::Approximate),
    (crate::keyboard_controller::NAME, Port::Gaps),
    (crate::limiter::NAME, Port::Exact),
    (crate::love_philter::NAME, Port::Gaps),
    (crate::low_lifter::NAME, Port::Exact),
    (crate::luxeverb::NAME, Port::Gaps),
    (crate::maximus::NAME, Port::Exact),
    (crate::minisynth::NAME, Port::Gaps),
    (crate::multiband_compressor::NAME, Port::Exact),
    (crate::notebook::NAME, Port::Exact),
    (crate::notebook_2::NAME, Port::Exact),
    (crate::morphine::NAME, Port::Gaps),
    (crate::ogun::NAME, Port::Approximate),
    (crate::panomatic::NAME, Port::Exact),
    (crate::parametric_eq::NAME, Port::Exact),
    (crate::parametric_eq_2::NAME, Port::Gaps),
    (crate::patcher::NAME, Port::Gaps),
    (crate::peak_controller::NAME, Port::Gaps),
    (crate::phaser::NAME, Port::Exact),
    (crate::pitch_shifter::NAME, Port::Gaps),
    (crate::pitcher::NAME, Port::Gaps),
    (crate::razer_chroma::NAME, Port::Exact),
    (crate::reeverb::NAME, Port::Exact),
    (crate::reeverb_2::NAME, Port::Exact),
    (crate::sakura::NAME, Port::Gaps),
    (crate::send::NAME, Port::Gaps),
    (crate::seven_band_eq::NAME, Port::Exact),
    (crate::slicex::NAME, Port::Approximate),
    (crate::soft_clipper::NAME, Port::Exact),
    (crate::soundgoodizer::NAME, Port::Gaps),
    (crate::spreader::NAME, Port::Exact),
    (crate::squeeze::NAME, Port::Exact),
    (crate::stereo_enhancer::NAME, Port::Exact),
    (crate::stereo_shaper::NAME, Port::Exact),
    (crate::sytrus::NAME, Port::Gaps),
    (crate::three_osc::NAME, Port::Gaps),
    (crate::transient_processor::NAME, Port::Exact),
    (crate::vintage_chorus::NAME, Port::Gaps),
    (crate::vintage_phaser::NAME, Port::Gaps),
    (crate::vocodex::NAME, Port::Gaps),
    (crate::wave_candy::NAME, Port::Exact),
    (crate::waveshaper::NAME, Port::Gaps),
    (crate::wrapper::NAME, Port::Gaps),
    (crate::xy_controller::NAME, Port::Gaps),
    (crate::xyz_controller::NAME, Port::Gaps),
    (crate::zgameeditor::NAME, Port::Gaps),
    (crate::wave_traveller::NAME, Port::Gaps),
];

/// The port of a plugin by its event 201 name, in any case.
pub fn port(name: &str) -> Option<Port> {
    let entry = PORTS
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name));
    entry.map(|(_, port)| *port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_is_a_hosted_plugin_named_once() {
        for (name, _) in PORTS {
            let hosted = crate::by_name(name).is_some() || crate::generator_by_name(name).is_some();
            assert!(hosted, "{name} has a port but no plugin");
        }
        let mut names: Vec<_> = PORTS.iter().map(|(name, _)| name.to_lowercase()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), PORTS.len(), "a plugin is named twice");
        // Nothing is null-tested against FL yet, so nothing can claim it.
        assert!(PORTS.iter().all(|(_, port)| *port != Port::Identical));
    }
}
