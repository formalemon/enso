//! Neovim as a theme target, through `rose-pine/neovim`.
//!
//! The plugin knows how every highlight group follows from the fifteen
//! Rosé Pine roles — which is what a theme is here — and takes a palette of
//! its own in place of its built-in ones. So theming Neovim is one generated
//! Lua file that hands the plugin the active palette, and one line in
//! `init.lua`, after the plugin is on the runtime path:
//!
//! ```lua
//! dofile(vim.fn.expand("~/.local/state/enso/nvim.lua"))
//! ```
//!
//! The file also watches for its own replacement, so every running Neovim
//! follows a theme switch. Without the plugin it still sets `background`.

use enso_core::Palette;

use super::{Job, Outcome, files, mentions};

/// Name of the generated file inside the state directory.
const FILE: &str = "nvim.lua";

pub fn apply(job: &Job<'_>) -> Result<Outcome, String> {
    let file = job.generate(FILE, &render(job))?;
    let host = job.host;
    let dir = host.config.join("nvim");
    if !dir.is_dir() && !host.has_program("nvim") {
        return Ok(Outcome::absent("not installed"));
    }
    // Configs are split over `lua/…` as a rule; look a few levels down.
    if mentions(
        &files(&dir, &["lua", "vim"], 4),
        "--",
        &format!("enso/{FILE}"),
    ) {
        return Ok(Outcome::done("colours updated"));
    }

    let load = format!("dofile(vim.fn.expand(\"{}\"))", host.tilde(&file));
    let vimscript = dir.join("init.vim").is_file() && !dir.join("init.lua").is_file();
    let (init, line) = if vimscript {
        ("init.vim", format!("lua {load}"))
    } else {
        ("init.lua", load)
    };
    Ok(Outcome::done("colours written").step(
        format!(
            "to use them, add to {}, after rose-pine/neovim is loaded:",
            host.tilde(&dir.join(init))
        ),
        [line],
    ))
}

/// A Lua file that applies the palette through the plugin, and reapplies it
/// whenever enso-ctl writes the next one.
pub fn render(job: &Job<'_>) -> String {
    let Palette {
        base,
        surface,
        overlay,
        muted,
        subtle,
        text,
        love,
        gold,
        rose,
        pine,
        foam,
        iris,
        highlight_low,
        highlight_med,
        highlight_high,
    } = job.colors();
    let dark = job.theme.is_dark();
    // The plugin's palettes come three to a name; the one of the right
    // brightness is overridden, so what is not a role here (`leaf`) fits.
    let (variant, background) = if dark {
        ("main", "dark")
    } else {
        ("dawn", "light")
    };
    // The background of windows that are not the current one, for those who
    // have the plugin dim them: the base, a touch further from the text.
    let dimmed = if dark {
        base.mix(enso_core::Rgba::rgb(0, 0, 0), 0.12)
    } else {
        base.mix(overlay, 0.35)
    };
    format!(
        r##"-- {banner}
--
-- The active enso theme, applied through rose-pine/neovim. Load it from
-- init.lua, once the plugin is on the runtime path:
--
--   dofile(vim.fn.expand("{path}"))

local palette = {{
  _nc = "{dimmed}",
  base = "{base}",
  surface = "{surface}",
  overlay = "{overlay}",
  muted = "{muted}",
  subtle = "{subtle}",
  text = "{text}",
  love = "{love}",
  gold = "{gold}",
  rose = "{rose}",
  pine = "{pine}",
  foam = "{foam}",
  iris = "{iris}",
  highlight_low = "{highlight_low}",
  highlight_med = "{highlight_med}",
  highlight_high = "{highlight_high}",
}}

-- Options given here are merged into the ones init.lua gave the plugin.
local ok, rose_pine = pcall(require, "rose-pine")
if ok then
  rose_pine.setup({{ variant = "{variant}", palette = {{ {variant} = palette }} }})
end
vim.o.background = "{background}"
if ok then
  vim.cmd.colorscheme("rose-pine")
end

-- enso-ctl replaces this file on every theme switch: watch its directory
-- for that, once per Neovim, and run the new one.
if not _G.enso_theme_watch then
  local uv = vim.uv or vim.loop
  local file = debug.getinfo(1, "S").source:sub(2)
  local dir, name = vim.fs.dirname(file), vim.fs.basename(file)
  local watch = uv.new_fs_event()
  local started = watch
    and watch:start(dir, {{}}, function(err, changed)
      if not err and changed == name then
        vim.schedule(function()
          pcall(dofile, file)
        end)
      end
    end)
  if started then
    _G.enso_theme_watch = watch
  end
end
"##,
        banner = job.banner(),
        path = job.host.tilde(&job.generated(FILE)),
    )
}

#[cfg(test)]
mod tests {
    use super::{super::sandbox::Sandbox, *};

    #[test]
    fn hands_the_plugin_the_palette() {
        let sandbox = Sandbox::new("nvim-render");
        let moon = render(&sandbox.job("rose-pine-moon"));
        assert!(moon.starts_with("-- Written by `enso-ctl theme set` — Rosé Pine Moon"));
        assert!(moon.contains(r#"dofile(vim.fn.expand("~/.local/state/enso/nvim.lua"))"#));
        assert!(moon.contains(r##"  base = "#232136","##), "{moon}");
        assert!(moon.contains(r##"  highlight_high = "#56526e","##));
        // The plugin's own value for Moon is #1f1d30: within a shade.
        assert!(moon.contains(r##"  _nc = "#1f1d30","##), "{moon}");
        assert!(
            moon.contains(r#"rose_pine.setup({ variant = "main", palette = { main = palette } })"#)
        );
        assert!(moon.contains(r#"vim.o.background = "dark""#));

        let dawn = render(&sandbox.job("rose-pine-dawn"));
        assert!(dawn.contains(r##"  base = "#faf4ed","##));
        assert!(
            dawn.contains(r#"rose_pine.setup({ variant = "dawn", palette = { dawn = palette } })"#)
        );
        assert!(dawn.contains(r#"vim.o.background = "light""#));
        // Balanced, and every role filled in.
        for text in [&moon, &dawn] {
            assert_eq!(text.matches('{').count(), text.matches('}').count());
            assert_eq!(text.matches("= \"#").count(), 16, "{text}");
        }
    }

    #[test]
    fn says_which_line_to_add_until_it_is_loaded() {
        let mut sandbox = Sandbox::new("nvim-apply");
        assert!(apply(&sandbox.job("rose-pine-moon")).unwrap().absent);
        assert!(
            sandbox
                .read(".local/state/enso/nvim.lua")
                .contains("#232136")
        );

        sandbox.install("nvim");
        let step = apply(&sandbox.job("rose-pine-moon")).unwrap().step.unwrap();
        assert_eq!(
            step.what,
            "to use them, add to ~/.config/nvim/init.lua, after rose-pine/neovim is loaded:"
        );
        assert_eq!(
            step.lines,
            [r#"dofile(vim.fn.expand("~/.local/state/enso/nvim.lua"))"#]
        );

        // A Vimscript config is told how to say it in Vimscript.
        sandbox.write(".config/nvim/init.vim", "set number\n");
        let step = apply(&sandbox.job("rose-pine-moon")).unwrap().step.unwrap();
        assert!(step.what.contains("~/.config/nvim/init.vim"), "{step:?}");
        assert!(step.lines[0].starts_with("lua dofile("), "{step:?}");

        // Loaded from a module of the config, a comment not counting.
        let module = ".config/nvim/lua/me/look.lua";
        sandbox.write(module, "-- dofile(\"~/.local/state/enso/nvim.lua\")\n");
        assert!(
            apply(&sandbox.job("rose-pine-moon"))
                .unwrap()
                .step
                .is_some()
        );
        sandbox.write(
            module,
            "dofile(vim.fn.expand(\"~/.local/state/enso/nvim.lua\"))\n",
        );
        assert_eq!(
            apply(&sandbox.job("rose-pine-moon")).unwrap(),
            Outcome::done("colours updated")
        );
    }
}
