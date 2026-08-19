# The guest build context is shipped with the application, because Cabin
# builds the guest image on the machine that runs it. Without these files a
# fresh installation has an application that cannot make a desktop.
install_data(
  'Containerfile',
  install_dir: datadir / 'cabin' / 'guest',
)

install_data(
  'scripts' / 'configure-rdp.sh',
  'scripts' / 'prepare.sh',
  'scripts' / 'start-shell.sh',
  'scripts' / 'unlock-keyring.sh',
  install_dir: datadir / 'cabin' / 'guest' / 'scripts',
)

install_data(
  'systemd' / 'system' / 'cabin-host-socket.service',
  'systemd' / 'system' / 'cabin-host-socket.socket',
  'systemd' / 'system' / 'cabin-prepare.service',
  install_dir: datadir / 'cabin' / 'guest' / 'systemd' / 'system',
)

install_data(
  'systemd' / 'system' / 'user@.service.d' / '10-cabin-runtime-dir.conf',
  install_dir: datadir / 'cabin' / 'guest' / 'systemd' / 'system' / 'user@.service.d',
)

install_data(
  'systemd' / 'user' / 'cabin-flathub.service',
  'systemd' / 'user' / 'cabin-keyring.service',
  'systemd' / 'user' / 'cabin-rdp.service',
  'systemd' / 'user' / 'cabin-session.target',
  'systemd' / 'user' / 'cabin-shell.service',
  install_dir: datadir / 'cabin' / 'guest' / 'systemd' / 'user',
)
