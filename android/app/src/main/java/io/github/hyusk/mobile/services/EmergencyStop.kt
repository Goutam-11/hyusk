package io.github.hyusk.mobile.services

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/** Shared process-local stop gate. Every action executor must check this before acting. */
object EmergencyStop {
    private val _engaged = MutableStateFlow(false)
    val engaged: StateFlow<Boolean> = _engaged
    fun engage() { _engaged.value = true }
    fun reset() { _engaged.value = false }
    fun allowAction(): Boolean = !_engaged.value
}
