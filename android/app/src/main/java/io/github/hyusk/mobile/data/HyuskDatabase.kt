package io.github.hyusk.mobile.data

import android.content.Context
import androidx.room.Database
import androidx.room.Dao
import androidx.room.Entity
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.Query
import androidx.room.Room
import androidx.room.RoomDatabase
import androidx.room.migration.Migration
import androidx.sqlite.db.SupportSQLiteDatabase
import kotlinx.coroutines.flow.Flow

@Entity(tableName = "workflows")
data class WorkflowEntity(
    @androidx.room.PrimaryKey val id: String,
    val name: String,
    val encryptedDefinition: String,
    val scope: String = "global",
    val originDevice: String,
    val baseRevision: Long = 0,
    val serverRevision: Long = 0,
    val deleted: Boolean = false,
    val conflicted: Boolean = false,
    val enabled: Boolean = true,
    val updatedAt: Long = System.currentTimeMillis()
)

@Entity(tableName = "memories")
data class MemoryEntity(
    @androidx.room.PrimaryKey val id: String,
    val kind: String,
    val encryptedPayload: String,
    val scope: String = "global",
    val originDevice: String,
    val baseRevision: Long = 0,
    val serverRevision: Long = 0,
    val deleted: Boolean = false,
    val conflicted: Boolean = false,
    val createdAt: Long = System.currentTimeMillis()
)

@Entity(tableName = "agent_turns")
data class AgentTurnEntity(
    @androidx.room.PrimaryKey(autoGenerate = true) val id: Long = 0,
    val role: String,
    val encryptedContent: String,
    val createdAt: Long = System.currentTimeMillis(),
)

@Entity(tableName = "agent_runs")
data class AgentRunEntity(
    @androidx.room.PrimaryKey val id: String,
    val encryptedState: String,
    val status: String,
    val updatedAt: Long = System.currentTimeMillis(),
)

@Dao
interface WorkflowDao {
    @Query("SELECT * FROM workflows WHERE deleted = 0 ORDER BY updatedAt DESC") fun observe(): Flow<List<WorkflowEntity>>
    @Query("SELECT * FROM workflows WHERE deleted = 0 AND enabled = 1 ORDER BY updatedAt DESC") suspend fun enabled(): List<WorkflowEntity>
    @Insert(onConflict = OnConflictStrategy.ABORT) suspend fun insert(workflow: WorkflowEntity)
    @Query("SELECT * FROM workflows WHERE id = :id") suspend fun get(id: String): WorkflowEntity?
    @androidx.room.Update suspend fun update(workflow: WorkflowEntity)
    @Query("UPDATE workflows SET enabled = :enabled WHERE id = :id") suspend fun setEnabled(id: String, enabled: Boolean)
}

@Dao
interface MemoryDao {
    @Query("SELECT * FROM memories WHERE deleted = 0 ORDER BY createdAt DESC") fun observe(): Flow<List<MemoryEntity>>
    @Insert(onConflict = OnConflictStrategy.ABORT) suspend fun insert(memory: MemoryEntity)
    @Query("SELECT * FROM memories WHERE id = :id") suspend fun get(id: String): MemoryEntity?
    @androidx.room.Update suspend fun update(memory: MemoryEntity)
    @Query("SELECT * FROM memories WHERE deleted = 0 ORDER BY createdAt DESC LIMIT :limit") suspend fun latest(limit: Int): List<MemoryEntity>
    @Query("DELETE FROM memories") suspend fun deleteAll()
}

@Dao
interface AgentTurnDao {
    @Insert suspend fun insert(turn: AgentTurnEntity)
    @Query("SELECT * FROM agent_turns ORDER BY createdAt DESC, id DESC LIMIT :limit") suspend fun latest(limit: Int): List<AgentTurnEntity>
    @Query("DELETE FROM agent_turns WHERE id NOT IN (SELECT id FROM agent_turns ORDER BY createdAt DESC, id DESC LIMIT :keep)") suspend fun trim(keep: Int)
}

@Dao
interface AgentRunDao {
    @Insert(onConflict = OnConflictStrategy.REPLACE) suspend fun save(run: AgentRunEntity)
    @Query("SELECT * FROM agent_runs ORDER BY updatedAt DESC LIMIT 100") fun observe(): Flow<List<AgentRunEntity>>
    @Query("SELECT * FROM agent_runs WHERE id = :id") suspend fun get(id: String): AgentRunEntity?
    @Query("SELECT * FROM agent_runs WHERE status = 'active' ORDER BY updatedAt DESC LIMIT 1") suspend fun latestActive(): AgentRunEntity?
    @Query("SELECT * FROM agent_runs WHERE status = 'awaiting_user' ORDER BY updatedAt DESC LIMIT 1") suspend fun latestAwaitingUser(): AgentRunEntity?
    @Query("SELECT * FROM agent_runs WHERE status IN ('paused', 'interrupted') ORDER BY updatedAt DESC LIMIT 1") suspend fun latestPaused(): AgentRunEntity?
    @Query("UPDATE agent_runs SET status = :status, updatedAt = :updatedAt WHERE id = :id") suspend fun setStatus(id: String, status: String, updatedAt: Long = System.currentTimeMillis())
    @Query("UPDATE agent_runs SET status = 'interrupted' WHERE status = 'active' AND updatedAt < :cutoff")
    suspend fun markStaleInterrupted(cutoff: Long)
    @Query("UPDATE agent_runs SET status = 'paused' WHERE status = 'active'")
    suspend fun pauseOrphanedActiveRuns()
}

@Database(entities = [WorkflowEntity::class, MemoryEntity::class, AgentTurnEntity::class, AgentRunEntity::class], version = 3, exportSchema = false)
abstract class HyuskDatabase : RoomDatabase() {
    abstract fun workflows(): WorkflowDao
    abstract fun memories(): MemoryDao
    abstract fun agentTurns(): AgentTurnDao
    abstract fun agentRuns(): AgentRunDao

    companion object {
        private val migration1To2 = object : Migration(1, 2) {
            override fun migrate(database: SupportSQLiteDatabase) {
                database.execSQL("CREATE TABLE IF NOT EXISTS agent_turns (id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, role TEXT NOT NULL, encryptedContent TEXT NOT NULL, createdAt INTEGER NOT NULL)")
            }
        }

        private val migration2To3 = object : Migration(2, 3) {
            override fun migrate(database: SupportSQLiteDatabase) {
                database.execSQL("CREATE TABLE IF NOT EXISTS agent_runs (id TEXT NOT NULL PRIMARY KEY, encryptedState TEXT NOT NULL, status TEXT NOT NULL, updatedAt INTEGER NOT NULL)")
            }
        }

        fun create(context: Context): HyuskDatabase = Room.databaseBuilder(context, HyuskDatabase::class.java, "hyusk.db")
            .addMigrations(migration1To2, migration2To3)
            .build()
    }
}
