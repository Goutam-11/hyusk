package io.github.hyusk.mobile.services

import android.graphics.drawable.Icon
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import io.github.hyusk.mobile.R

class HyuskQuickSettingsTileService : TileService() {
    override fun onStartListening() { updateTile() }
    override fun onClick() {
        if (EmergencyStop.engaged.value) EmergencyStop.reset() else EmergencyStop.engage()
        updateTile()
    }
    private fun updateTile() {
        qsTile?.apply {
            state = if (EmergencyStop.engaged.value) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
            label = if (EmergencyStop.engaged.value) "Hyusk stopped" else "Hyusk ready"
            contentDescription = label
            icon = Icon.createWithResource(this@HyuskQuickSettingsTileService, R.drawable.ic_hyusk)
            updateTile()
        }
    }
}
