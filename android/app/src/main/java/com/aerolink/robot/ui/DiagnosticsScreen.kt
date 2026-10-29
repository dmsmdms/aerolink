package com.aerolink.robot.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import com.aerolink.robot.BuildConfig
import com.aerolink.robot.R
import com.aerolink.robot.link.Caps
import com.aerolink.robot.link.DriveSource
import com.aerolink.robot.link.Faults
import com.aerolink.robot.link.LinkPhase
import com.aerolink.robot.link.LinkStatus
import com.aerolink.robot.link.RobotLink
import com.aerolink.robot.link.RobotState
import com.aerolink.robot.power.ChargeControl
import com.aerolink.robot.power.ChargeStatus
import com.aerolink.robot.service.DeviceOwner
import com.aerolink.robot.service.RobotConfig
import com.aerolink.robot.service.RobotHub
import com.aerolink.robot.service.RobotSettings
import com.aerolink.robot.signal.SignalPhase
import com.aerolink.robot.signal.SignalState
import com.aerolink.robot.teleop.PeerState
import com.aerolink.robot.teleop.TeleopStatus
import com.aerolink.robot.update.FirmwareStatus
import com.aerolink.robot.update.UpdateStatus
import kotlinx.coroutines.flow.MutableStateFlow
import kotlin.math.hypot

/** Bench diagnostics: server connection, operator session, link state, telemetry, ARM/DISARM, local joystick, charge window. */
@Composable
fun DiagnosticsScreen(onSaveConfig: (RobotConfig) -> Unit) {
    val link by RobotHub.link.collectAsState()
    val charge by RobotHub.charge.collectAsState()
    val signal by RobotHub.signal.collectAsState()
    val teleop by RobotHub.teleop.collectAsState()
    val update by RobotHub.update.collectAsState()
    val firmware by RobotHub.firmware.collectAsState()
    val statusFlow = remember(link) { link?.status ?: MutableStateFlow<LinkStatus?>(null) }
    val status by statusFlow.collectAsState()

    Surface(modifier = Modifier.fillMaxSize()) {
        Column(
            modifier = Modifier.verticalScroll(rememberScrollState()).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Image(
                painter = painterResource(R.drawable.logo),
                contentDescription = stringResource(R.string.app_name),
                modifier = Modifier.height(56.dp),
            )
            Text(stringResource(R.string.version, BuildConfig.VERSION_NAME), style = MaterialTheme.typography.titleMedium)
            ServerSection(signal, teleop, onSaveConfig)
            LinkSection(link, status, firmware)
            ChargeSection(charge)
            AppSection(update)
        }
    }
}

@Composable
private fun ServerSection(signal: SignalState?, teleop: TeleopStatus?, onSave: (RobotConfig) -> Unit) {
    val context = LocalContext.current
    val saved = remember { RobotSettings.load(context) }
    var server by remember { mutableStateOf(saved.server) }
    var token by remember { mutableStateOf(saved.token) }
    var simulate by remember { mutableStateOf(saved.simulateBase) }
    var rotation by remember { mutableStateOf(saved.videoRotation) }

    Text(stringResource(R.string.section_server), style = MaterialTheme.typography.titleSmall)
    Text(
        when (signal?.phase) {
            null -> stringResource(R.string.server_not_configured)
            SignalPhase.CONNECTING -> stringResource(R.string.server_connecting)
            SignalPhase.ONLINE -> stringResource(R.string.server_online)
            SignalPhase.OFFLINE -> stringResource(R.string.server_offline, signal.error ?: "")
            SignalPhase.UNAUTHORIZED -> stringResource(R.string.server_unauthorized)
        },
    )
    if (teleop?.active == true) {
        val peer = teleop.peer?.let { stringResource(peerText(it)) } ?: stringResource(R.string.peer_waiting)
        Text(stringResource(R.string.session_operator, teleop.operator.orEmpty(), peer))
    } else if (signal?.phase == SignalPhase.ONLINE) {
        Text(stringResource(R.string.session_none))
    }
    OutlinedTextField(
        value = server,
        onValueChange = { server = it },
        label = { Text(stringResource(R.string.label_server_url)) },
        placeholder = { Text("https://robots.example.org") },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
        modifier = Modifier.fillMaxWidth(),
    )
    OutlinedTextField(
        value = token,
        onValueChange = { token = it },
        label = { Text(stringResource(R.string.label_robot_token)) },
        singleLine = true,
        visualTransformation = PasswordVisualTransformation(),
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
        modifier = Modifier.fillMaxWidth(),
    )
    if (BuildConfig.DEBUG) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Switch(checked = simulate, onCheckedChange = { simulate = it })
            Text(stringResource(R.string.simulate_base))
        }
    }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(stringResource(R.string.label_video_rotation))
        OutlinedButton(onClick = { rotation = (rotation + 90) % 360 }) { Text("$rotation°") }
    }
    val config = RobotConfig(server.trim(), token.trim(), simulate, rotation)
    Button(onClick = { onSave(config) }, enabled = config.server.isEmpty() || config.socketUrl != null) {
        Text(stringResource(R.string.button_save))
    }
}

@Composable
private fun LinkSection(link: RobotLink?, status: LinkStatus?, firmware: FirmwareStatus?) {
    Text(stringResource(R.string.section_link), style = MaterialTheme.typography.titleSmall)
    if (link == null || status == null) {
        Text(stringResource(R.string.no_link))
        return
    }
    Text(stringResource(phaseText(status.phase)))
    status.firmware?.let { fw ->
        Text(stringResource(R.string.label_firmware, fw.firmwareVersion, fw.protocolVersion, fw.firmwareCrc))
        Text(stringResource(R.string.label_board, "%016X".format(fw.boardId)))
        val motor = if (fw.caps and Caps.MOTOR_SIMULATED != 0) R.string.motor_simulated else R.string.motor_hardware
        Text(stringResource(R.string.label_motor, stringResource(motor)))
    }
    firmware?.let { Text(firmwareText(it)) }
    if (status.phase != LinkPhase.READY) return

    Text(stringResource(R.string.label_state, stringResource(stateText(status.robotState))))
    Text(stringResource(R.string.label_faults, faultsText(status.faults)))
    status.telemetry?.let { t ->
        Text(stringResource(R.string.label_wheels, t.speedLeft, t.speedRight))
        val battery = t.batteryMv?.let { stringResource(R.string.volts, it / 1000.0) }
            ?: stringResource(R.string.unknown)
        Text(stringResource(R.string.label_robot_battery, battery))
        Text(stringResource(R.string.label_errors, t.rxErrors, status.decodeErrors))
    }
    status.lastNack?.let { Text(stringResource(R.string.label_nack, it.rejectedType, it.reason)) }

    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Button(onClick = link::arm, enabled = status.robotState == RobotState.DISARMED) {
            Text(stringResource(R.string.button_arm))
        }
        OutlinedButton(onClick = link::disarm) {
            Text(stringResource(R.string.button_disarm))
        }
    }
    Text(stringResource(R.string.joystick_hint))
    Joystick(enabled = status.robotState == RobotState.ARMED) { speed, steer ->
        link.setDrive(speed, steer, DriveSource.LOCAL)
    }
    // Leaving the screen must not leave a wish behind (the stream would keep it alive).
    DisposableEffect(link) {
        onDispose { link.setDrive(0, 0, DriveSource.LOCAL) }
    }
}

/** ROB-009. */
@Composable
private fun firmwareText(firmware: FirmwareStatus): String = when (firmware) {
    FirmwareStatus.NoBase -> stringResource(R.string.firmware_no_base)
    FirmwareStatus.NoImage -> stringResource(R.string.firmware_no_image)
    FirmwareStatus.Current -> stringResource(R.string.firmware_current)
    FirmwareStatus.Waiting -> stringResource(R.string.firmware_waiting)
    is FirmwareStatus.Sending -> stringResource(R.string.firmware_sending, firmware.percent)
    FirmwareStatus.Installing -> stringResource(R.string.firmware_installing)
    is FirmwareStatus.Failed -> stringResource(R.string.firmware_failed, firmware.error)
}

@Composable
private fun ChargeSection(charge: ChargeStatus?) {
    Text(stringResource(R.string.section_phone), style = MaterialTheme.typography.titleSmall)
    if (charge == null) {
        Text(stringResource(R.string.unknown))
        return
    }
    if (charge.levelPercent != null && charge.tempDeciC != null) {
        val plugged = stringResource(if (charge.pluggedIn) R.string.plugged else R.string.unplugged)
        Text(stringResource(R.string.label_phone_battery, charge.levelPercent, charge.tempDeciC / 10.0, plugged))
    }
    val control = when (charge.control) {
        ChargeControl.PROBING -> stringResource(R.string.charge_probing)
        ChargeControl.UNAVAILABLE -> stringResource(R.string.charge_unavailable)
        ChargeControl.ACTIVE -> stringResource(
            R.string.charge_active,
            charge.switchPath.orEmpty(),
            stringResource(if (charge.chargingAllowed) R.string.charge_allowed else R.string.charge_paused),
        )
    }
    Text(control)
}

/** ROB-008 updates and PWR-014 Device Owner. */
@Composable
private fun AppSection(update: UpdateStatus?) {
    val context = LocalContext.current
    var owner by remember { mutableStateOf(DeviceOwner.isOwner(context)) }
    Text(stringResource(R.string.section_app), style = MaterialTheme.typography.titleSmall)
    Text(stringResource(R.string.label_app_version, BuildConfig.VERSION_NAME, BuildConfig.VERSION_CODE))
    Text(
        when (update) {
            null, UpdateStatus.Current -> stringResource(R.string.update_current)
            is UpdateStatus.Blocked -> stringResource(R.string.update_blocked, update.version)
            is UpdateStatus.Waiting -> stringResource(R.string.update_waiting, update.version)
            is UpdateStatus.Downloading -> stringResource(R.string.update_downloading, update.version, update.percent)
            is UpdateStatus.Installing -> stringResource(R.string.update_installing, update.version)
            is UpdateStatus.Failed -> stringResource(R.string.update_failed, update.version, update.error)
        },
    )
    Text(stringResource(if (owner) R.string.device_owner_yes else R.string.device_owner_no))
    if (owner) {
        OutlinedButton(onClick = {
            DeviceOwner.release(context)
            owner = DeviceOwner.isOwner(context)
        }) { Text(stringResource(R.string.button_release_owner)) }
    }
}

/** Speed (up = forward) and steer (right = +) in ‰; released → zero. */
@Composable
private fun Joystick(enabled: Boolean, onChange: (speed: Int, steer: Int) -> Unit) {
    var knob by remember { mutableStateOf(Offset.Zero) } // normalized −1..1
    val color = MaterialTheme.colorScheme.primary.copy(alpha = if (enabled) 1f else 0.3f)

    Canvas(
        modifier = Modifier
            .size(240.dp)
            .padding(8.dp)
            .pointerInput(enabled) {
                if (!enabled) {
                    knob = Offset.Zero
                    return@pointerInput
                }
                val radius = size.width / 2f
                fun move(position: Offset) {
                    var v = (position - Offset(radius, radius)) / radius
                    val length = hypot(v.x, v.y)
                    if (length > 1f) v /= length
                    knob = v
                    onChange((-v.y * 1000).toInt(), (v.x * 1000).toInt())
                }
                fun release() {
                    knob = Offset.Zero
                    onChange(0, 0)
                }
                detectDragGestures(
                    onDragStart = ::move,
                    onDragEnd = ::release,
                    onDragCancel = ::release,
                ) { change, _ -> move(change.position) }
            },
    ) {
        val radius = size.minDimension / 2
        drawCircle(color, radius = radius, style = Stroke(width = 4f))
        drawCircle(color, radius = radius / 4, center = center + knob * radius)
    }
}

private fun peerText(state: PeerState) = when (state) {
    PeerState.NEW, PeerState.CONNECTING -> R.string.peer_connecting
    PeerState.CONNECTED -> R.string.peer_connected
    PeerState.DISCONNECTED -> R.string.peer_disconnected
    PeerState.FAILED -> R.string.peer_failed
    PeerState.CLOSED -> R.string.peer_closed
}

private fun phaseText(phase: LinkPhase) = when (phase) {
    LinkPhase.HANDSHAKE -> R.string.phase_handshake
    LinkPhase.READY -> R.string.phase_ready
    LinkPhase.INCOMPATIBLE -> R.string.phase_incompatible
    LinkPhase.NO_RESPONSE -> R.string.phase_no_response
    LinkPhase.CLOSED -> R.string.phase_closed
}

private fun stateText(state: RobotState) = when (state) {
    RobotState.DISARMED -> R.string.state_disarmed
    RobotState.ARMED -> R.string.state_armed
    RobotState.STOPPING -> R.string.state_stopping
    RobotState.FAULT -> R.string.state_fault
}

@Composable
private fun faultsText(faults: Int): String {
    if (faults == 0) return stringResource(R.string.faults_none)
    val names = listOf(
        Faults.MOTOR_LINK_LOST to R.string.fault_motor_link_lost,
        Faults.MOTOR_FAULT to R.string.fault_motor,
        Faults.LOW_BATTERY to R.string.fault_low_battery,
        Faults.OVERTEMP to R.string.fault_overtemp,
    ).filter { (bit, _) -> faults and bit != 0 }.map { (_, res) -> stringResource(res) }
    return names.joinToString(", ")
}
