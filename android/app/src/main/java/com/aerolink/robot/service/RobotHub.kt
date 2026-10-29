package com.aerolink.robot.service

import com.aerolink.robot.link.RobotLink
import com.aerolink.robot.power.ChargeStatus
import com.aerolink.robot.signal.SignalState
import com.aerolink.robot.teleop.TeleopStatus
import com.aerolink.robot.update.FirmwareStatus
import com.aerolink.robot.update.UpdateStatus
import kotlinx.coroutines.flow.MutableStateFlow

/** Process-wide handles published by [RobotLinkService] for the diagnostics UI. Main thread only. */
object RobotHub {
    val link = MutableStateFlow<RobotLink?>(null)
    val charge = MutableStateFlow<ChargeStatus?>(null)
    /** null: not provisioned (no server or token). */
    val signal = MutableStateFlow<SignalState?>(null)
    val teleop = MutableStateFlow<TeleopStatus?>(null)
    val update = MutableStateFlow<UpdateStatus?>(null)
    val firmware = MutableStateFlow<FirmwareStatus?>(null)
}
