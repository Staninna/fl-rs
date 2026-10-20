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
    (crate::bassdrum::NAME, Port::Exact),
    (crate::blood_overdrive::NAME, Port::Exact),
    (crate::boobass::NAME, Port::Exact),
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
    (crate::drumpad::NAME, Port::Gaps),
    (crate::dx10::NAME, Port::Gaps),
    (crate::effector::NAME, Port::Gaps),
    (crate::equo::NAME, Port::Gaps),
    (crate::fast_dist::NAME, Port::Exact),
    (crate::fast_lp::NAME, Port::Exact),
    (crate::filter::NAME, Port::Exact),
    (crate::fl_keys::NAME, Port::Gaps),
    (crate::flangus::NAME, Port::Exact),
    (crate::fpc::NAME, Port::Gaps),
    (crate::frequency_splitter::NAME, Port::Gaps),
    (crate::fruit_kick::NAME, Port::Gaps),
    (crate::gross_beat::NAME, Port::Gaps),
    (crate::harmless::NAME, Port::Approximate),
    (crate::harmor::NAME, Port::Approximate),
    (crate::html_notebook::NAME, Port::Exact),
    (crate::hyper_chorus::NAME, Port::Gaps),
    (crate::limiter::NAME, Port::Exact),
    (crate::love_philter::NAME, Port::Gaps),
    (crate::luxeverb::NAME, Port::Gaps),
    (crate::maximus::NAME, Port::Gaps),
    (crate::multiband_compressor::NAME, Port::Exact),
    (crate::notebook::NAME, Port::Exact),
    (crate::notebook_2::NAME, Port::Exact),
    (crate::ogun::NAME, Port::Approximate),
    (crate::panomatic::NAME, Port::Gaps),
    (crate::parametric_eq_2::NAME, Port::Gaps),
    (crate::peak_controller::NAME, Port::Gaps),
    (crate::pitch_shifter::NAME, Port::Gaps),
    (crate::pitcher::NAME, Port::Gaps),
    (crate::razer_chroma::NAME, Port::Exact),
    (crate::reeverb::NAME, Port::Exact),
    (crate::reeverb_2::NAME, Port::Exact),
    (crate::send::NAME, Port::Gaps),
    (crate::slicex::NAME, Port::Approximate),
    (crate::soft_clipper::NAME, Port::Exact),
    (crate::soundgoodizer::NAME, Port::Gaps),
    (crate::spreader::NAME, Port::Exact),
    (crate::squeeze::NAME, Port::Exact),
    (crate::stereo_enhancer::NAME, Port::Exact),
    (crate::stereo_shaper::NAME, Port::Exact),
    (crate::sytrus::NAME, Port::Approximate),
    (crate::three_osc::NAME, Port::Gaps),
    (crate::transient_processor::NAME, Port::Exact),
    (crate::vocodex::NAME, Port::Gaps),
    (crate::wave_candy::NAME, Port::Exact),
    (crate::waveshaper::NAME, Port::Gaps),
    (crate::wrapper::NAME, Port::Gaps),
    (crate::xy_controller::NAME, Port::Gaps),
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
