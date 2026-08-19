project('cabin',
  version: '0.1.0',
  meson_version: '>= 1.0.0',
  license: 'GPL-3.0-or-later',
)

app_id = 'io.github.jaydoubleu.Cabin'

cargo = find_program('cargo', required: true)
gnome = import('gnome')
i18n  = import('i18n')

prefix     = get_option('prefix')
bindir     = prefix / get_option('bindir')
datadir    = prefix / get_option('datadir')

conf = configuration_data()
conf.set('APP_ID',  app_id)
conf.set('VERSION', meson.project_version())

subdir('data')
subdir('po')

if get_option('buildtype') == 'release'
  rust_profile = 'release'
else
  rust_profile = 'debug'
endif

cargo_build = find_program('build-aux/cargo-build.sh')

custom_target('cabin',
  build_by_default: true,
  build_always_stale: true,
  output: 'cabin',
  console: true,
  install: true,
  install_dir: bindir,
  command: [
    cargo_build,
    meson.project_source_root(),
    meson.project_build_root(),
    rust_profile,
    app_id,
    '@OUTPUT@',
  ],
)

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
