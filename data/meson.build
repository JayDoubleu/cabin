desktop_file = i18n.merge_file(
  input: configure_file(
    input: 'cabin.desktop.in.in',
    output: '@0@.desktop.in'.format(app_id),
    configuration: conf,
  ),
  output: '@0@.desktop'.format(app_id),
  type: 'desktop',
  po_dir: '../po',
  install: true,
  install_dir: datadir / 'applications',
)

metainfo_file = i18n.merge_file(
  input: configure_file(
    input: 'cabin.metainfo.xml.in.in',
    output: '@0@.metainfo.xml.in'.format(app_id),
    configuration: conf,
  ),
  output: '@0@.metainfo.xml'.format(app_id),
  po_dir: '../po',
  install: true,
  install_dir: datadir / 'metainfo',
)

install_data(
  'cabin.gschema.xml',
  rename: '@0@.gschema.xml'.format(app_id),
  install_dir: datadir / 'glib-2.0' / 'schemas',
)

install_data(
  'icons' / 'hicolor' / 'scalable' / 'apps' / 'cabin.svg',
  rename: '@0@.svg'.format(app_id),
  install_dir: datadir / 'icons' / 'hicolor' / 'scalable' / 'apps',
)

# Validate what we ship, so a broken desktop or metainfo file fails the build.
desktop_utils = find_program('desktop-file-validate', required: false)
if desktop_utils.found()
  test('validate-desktop', desktop_utils, args: [desktop_file])
endif

appstream_util = find_program('appstreamcli', required: false)
if appstream_util.found()
  test('validate-metainfo', appstream_util,
       args: ['validate', '--no-net', '--explain', metainfo_file])
endif
