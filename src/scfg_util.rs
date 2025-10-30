// Copyright 2023-2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Utility functions for working with scfg directives.

use std::process::Stdio;

use anyhow::{Context, bail, ensure};
use scfg::{Directive, Scfg};

use crate::RawCommand;

/// Flatten a `Vec` which is expected to have a single item.
pub(crate) fn flatten_single_vec<T>(mut vec: Vec<T>) -> Option<T> {
    if vec.len() == 1 { vec.pop() } else { None }
}

/// Take a directive expecting it at most once.
///
/// # Errors
///
/// If the directive is defined more than once.
pub(crate) fn take_single_directive(
    config: &mut Scfg,
    name: &str,
) -> anyhow::Result<Option<Directive>> {
    if let Some(mut directives) = config.remove(name) {
        ensure!(
            directives.len() == 1,
            "{name} may only be specified once per block.",
        );
        let directive = directives
            .pop()
            .expect("directives contains exactly one element");
        Ok(Some(directive))
    } else {
        Ok(None)
    }
}

/// Take a single parameter from a directive.
///
/// # Errors
///
/// Returns an error if zero or more than one parameter is specified.
pub(crate) fn take_single_param_from_directive(
    config: &mut Scfg,
    name: &str,
) -> anyhow::Result<String> {
    let mut directive = take_single_directive(config, name)?
        .with_context(|| format!("directive {name} not found"))?;
    take_single_param(&mut directive).with_context(|| format!("Parsing directive {name}"))
}

/// Take a single parameter from a directive.
///
/// "single" here implies that it must not be followed by any other parameters.
pub(crate) fn take_single_param(directive: &mut Directive) -> anyhow::Result<String> {
    let mut params = directive.take_params().into_iter();
    let param = params.next().context("a parameter must be specified")?;
    if params.next().is_some() {
        bail!("no more than one parameter must be specified");
    }
    Ok(param)
}

/// Resolve a command in-place, updating the input structure.
///
/// Use to resolve a command early before the full structure is parsed into a domain type.
pub(crate) fn resolve_cmd_inplace(storage: &mut Scfg, name: &str) -> anyhow::Result<()> {
    let Some(mut directive) = take_single_directive(storage, name)? else {
        return Ok(());
    };

    let mut params = directive.take_params().into_iter();
    let value = if let Some(param) = params.next() {
        if params.next().is_some() {
            bail!("Found more than one parameter for directive {name}");
        }
        param
    } else {
        let mut block = directive
            .take_child()
            .context("Must define a parameter or a block")?;

        let raw_cmd = if let Some(mut cmd_block) = take_single_directive(&mut block, "cmd")? {
            let args = cmd_block.take_params().into_iter();
            RawCommand::from_args(args).context("cmd must define at least one parameter")?
        } else if let Some(mut shell_block) = take_single_directive(&mut block, "shell")? {
            let args = [
                "sh".to_string(),
                "-c".to_string(),
                shell_block.take_params().join(" "),
            ]
            .into_iter();
            RawCommand::from_args(args).context("shell must define at least one parameter")?
        } else {
            bail!("Block must include a 'cmd' or 'shell' directive");
        };

        let output = raw_cmd
            .command()
            .stdout(Stdio::piped())
            .output()
            .with_context(|| format!("Error executing command for {name} directive"))?;
        match output.status.code() {
            Some(0) => std::str::from_utf8(&output.stdout)?.trim().to_owned(),
            Some(code) => bail!("Command exited with status {code}."),
            None => bail!("Command exited unexpectedly."),
        }
    };

    let url_directive = storage.add(name);
    url_directive.append_param(value);

    Ok(())
}
