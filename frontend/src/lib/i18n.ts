// UI strings, English and Russian (no hard-coded user-facing text). Language: saved choice, else browser.

const en = {
  'app.title': 'AeroLink',
  'lang.switch': 'Русский',
  'landing.tagline': 'Teleoperated sidewalk delivery robots',
  'landing.text': 'Operators drive robots over the internet with live video. Robots ask for help when they cannot decide on their own.',
  'landing.console': 'Operator console',
  'landing.soon': 'Coming soon: operator registration and robot rental.',
  'login.title': 'Operator sign-in',
  'login.email': 'E-mail',
  'login.password': 'Password',
  'login.submit': 'Sign in',
  'login.invalid': 'Wrong e-mail or password.',
  'login.rate_limited': 'Too many attempts. Wait a minute and try again.',
  'login.failed': 'Sign-in failed. Check the connection and try again.',
  'header.logout': 'Sign out',
  'conn.connecting': 'Connecting…',
  'conn.open': 'Online',
  'conn.offline': 'Offline, reconnecting…',
  'robots.title': 'Robots',
  'robots.empty': 'No robots registered.',
  'robots.offline': 'offline',
  'robots.busy': 'controlled by another operator',
  'robots.mine': 'yours',
  'robots.take': 'Take',
  'robots.open': 'Open',
  'mode.IDLE': 'idle',
  'mode.TELEOP': 'teleoperated',
  'mode.AUTO': 'autopilot',
  'mode.NEEDS_HELP': 'needs help',
  'robot.release': 'Release',
  'robot.arm': 'Arm',
  'robot.disarm': 'Stop (Space)',
  'robot.mute': 'Mute',
  'robot.unmute': 'Unmute',
  'robot.talk': 'Talk (T)',
  'robot.talk_stop': 'Stop talking (T)',
  'robot.light_on': 'Light (L)',
  'robot.light_off': 'Light off (L)',
  'robot.mic_error': 'Microphone not available: {error}',
  'robot.no_video': 'Waiting for video…',
  'robot.connection': 'Link: {state}',
  'robot.path.direct': 'direct',
  'robot.path.relay': 'via TURN relay',
  'robot.rtt': 'Round trip: {ms} ms',
  'robot.state': 'State: {state}',
  'robot.faults': 'Faults: {faults}',
  'robot.wheels': 'Wheels L/R: {left} / {right} ‰',
  'robot.battery': 'Robot battery: {volts} V',
  'robot.phone_battery': 'Phone battery: {percent} %',
  'robot.telemetry_stale': 'No telemetry',
  'robot.no_base': 'Robot base (motors) not connected: video only',
  'robot.drive': 'Drive: speed {speed} ‰, steer {steer} ‰',
  'robot.help': 'W/S or ↑/↓ — speed, A/D or ←/→ — steer, Shift — full speed, Space — stop, T — talk through the robot, L — light. Gamepad: left stick — speed, right stick — steer, B — stop.',
  'pc.new': 'new',
  'pc.connecting': 'connecting',
  'pc.connected': 'connected',
  'pc.disconnected': 'interrupted',
  'pc.failed': 'failed',
  'pc.closed': 'closed',
  'state.DISARMED': 'disarmed',
  'state.ARMED': 'armed',
  'state.STOPPING': 'stopping',
  'state.FAULT': 'fault',
  'faults.none': 'none',
  'fault.motor_link_lost': 'motor link lost',
  'fault.motor': 'motor fault',
  'fault.low_battery': 'low battery',
  'fault.overtemp': 'overheating',
  'error.not_found': 'Robot not found.',
  'error.offline': 'The robot is offline.',
  'error.busy': 'Another operator controls this robot.',
  'error.no_session': 'The session has ended.',
  'end.released': 'Session released.',
  'end.robot_ended': 'The robot ended the session.',
  'end.robot_offline': 'The robot went offline.',
  'end.operator_offline': 'Connection to the server lost.',
  'notice.dismiss': 'Dismiss',
} as const;

export type Key = keyof typeof en;

const ru: Record<Key, string> = {
  'app.title': 'AeroLink',
  'lang.switch': 'English',
  'landing.tagline': 'Роботы-доставщики под управлением операторов',
  'landing.text': 'Операторы ведут роботов через интернет по живому видео. Робот зовёт оператора, когда не может решить сам.',
  'landing.console': 'Консоль оператора',
  'landing.soon': 'Скоро: регистрация операторов и аренда роботов.',
  'login.title': 'Вход для операторов',
  'login.email': 'E-mail',
  'login.password': 'Пароль',
  'login.submit': 'Войти',
  'login.invalid': 'Неверный e-mail или пароль.',
  'login.rate_limited': 'Слишком много попыток. Подождите минуту и повторите.',
  'login.failed': 'Не удалось войти. Проверьте связь и повторите.',
  'header.logout': 'Выйти',
  'conn.connecting': 'Подключение…',
  'conn.open': 'На связи',
  'conn.offline': 'Нет связи, переподключение…',
  'robots.title': 'Роботы',
  'robots.empty': 'Нет зарегистрированных роботов.',
  'robots.offline': 'не в сети',
  'robots.busy': 'управляет другой оператор',
  'robots.mine': 'ваш',
  'robots.take': 'Взять',
  'robots.open': 'Открыть',
  'mode.IDLE': 'стоит',
  'mode.TELEOP': 'под управлением',
  'mode.AUTO': 'автопилот',
  'mode.NEEDS_HELP': 'нужна помощь',
  'robot.release': 'Отпустить',
  'robot.arm': 'Включить',
  'robot.disarm': 'Стоп (пробел)',
  'robot.mute': 'Выключить звук',
  'robot.unmute': 'Включить звук',
  'robot.talk': 'Говорить (T)',
  'robot.talk_stop': 'Замолчать (T)',
  'robot.light_on': 'Свет (L)',
  'robot.light_off': 'Выключить свет (L)',
  'robot.mic_error': 'Микрофон недоступен: {error}',
  'robot.no_video': 'Ожидание видео…',
  'robot.connection': 'Связь: {state}',
  'robot.path.direct': 'напрямую',
  'robot.path.relay': 'через TURN',
  'robot.rtt': 'Задержка туда-обратно: {ms} мс',
  'robot.state': 'Состояние: {state}',
  'robot.faults': 'Неисправности: {faults}',
  'robot.wheels': 'Колёса Л/П: {left} / {right} ‰',
  'robot.battery': 'Батарея робота: {volts} В',
  'robot.phone_battery': 'Батарея телефона: {percent} %',
  'robot.telemetry_stale': 'Нет телеметрии',
  'robot.no_base': 'База робота (моторы) не подключена: только видео',
  'robot.drive': 'Движение: скорость {speed} ‰, поворот {steer} ‰',
  'robot.help': 'W/S или ↑/↓ — скорость, A/D или ←/→ — поворот, Shift — полный ход, пробел — стоп, T — говорить через робота, L — свет. Геймпад: левый стик — скорость, правый — поворот, B — стоп.',
  'pc.new': 'новая',
  'pc.connecting': 'соединение',
  'pc.connected': 'установлена',
  'pc.disconnected': 'прервана',
  'pc.failed': 'ошибка',
  'pc.closed': 'закрыта',
  'state.DISARMED': 'выключен',
  'state.ARMED': 'включён',
  'state.STOPPING': 'остановка',
  'state.FAULT': 'неисправность',
  'faults.none': 'нет',
  'fault.motor_link_lost': 'нет связи с моторами',
  'fault.motor': 'ошибка моторов',
  'fault.low_battery': 'низкий заряд',
  'fault.overtemp': 'перегрев',
  'error.not_found': 'Робот не найден.',
  'error.offline': 'Робот не в сети.',
  'error.busy': 'Этим роботом управляет другой оператор.',
  'error.no_session': 'Сессия завершена.',
  'end.released': 'Сессия завершена.',
  'end.robot_ended': 'Робот завершил сессию.',
  'end.robot_offline': 'Робот отключился.',
  'end.operator_offline': 'Потеряна связь с сервером.',
  'notice.dismiss': 'Закрыть',
};

export const dictionaries = { en, ru } as const;
export type Lang = keyof typeof dictionaries;

const STORAGE_KEY = 'aerolink.lang';

function detect(): Lang {
  const saved = typeof localStorage === 'undefined' ? null : localStorage.getItem(STORAGE_KEY);
  if (saved === 'en' || saved === 'ru') return saved;
  const browser = typeof navigator === 'undefined' ? 'en' : navigator.language;
  return browser.toLowerCase().startsWith('ru') ? 'ru' : 'en';
}

export const lang: Lang = detect();

export function t(key: Key, params: Record<string, string | number> = {}): string {
  return dictionaries[lang][key].replace(/\{(\w+)\}/g, (_, name: string) => String(params[name] ?? `{${name}}`));
}

/** Switching reloads the page: strings are read once, nothing has to be reactive. */
export function toggleLang(): void {
  localStorage.setItem(STORAGE_KEY, lang === 'en' ? 'ru' : 'en');
  location.reload();
}

const FAULTS: [number, Key][] = [
  [1 << 0, 'fault.motor_link_lost'],
  [1 << 1, 'fault.motor'],
  [1 << 2, 'fault.low_battery'],
  [1 << 3, 'fault.overtemp'],
];

/** Fault bits as in docs/03-usb-link.md. */
export function faultsText(faults: number): string {
  if (!faults) return t('faults.none');
  return FAULTS.filter(([bit]) => faults & bit).map(([, key]) => t(key)).join(', ');
}
