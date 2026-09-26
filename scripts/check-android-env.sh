#!/usr/bin/env bash
set -u

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sdk_dir="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Android/Sdk}}"
failed=0

check_command() {
    local label="$1"
    local command_name="$2"
    if command -v "$command_name" >/dev/null 2>&1; then
        printf '[ok] %-18s %s\n' "$label" "$(command -v "$command_name")"
    else
        printf '[missing] %-13s %s\n' "$label" "$command_name"
        failed=1
    fi
}

printf 'Hyusk Android environment\n\n'
check_command "Java" java
if command -v java >/dev/null 2>&1; then
    java_major="$(java -version 2>&1 | sed -n 's/.*version "\([0-9][0-9]*\).*/\1/p' | head -1)"
    if [[ "$java_major" != "17" ]]; then
        printf '[warning] Java version   %s detected; this project is pinned to JDK 17 (AGP 8.7/Gradle 8.9)\n' "${java_major:-unknown}"
        if [[ -d /usr/lib/jvm/java-17-openjdk ]]; then
            printf '          Use: export JAVA_HOME=/usr/lib/jvm/java-17-openjdk\n'
        else
            printf '          Install Fedora java-17-openjdk-devel or use a temporary JDK 17 build helper.\n'
        fi
    else
        printf '[ok] Java version      JDK 17\n'
    fi
fi

if [[ -d "$sdk_dir" ]]; then
    printf '[ok] Android SDK        %s\n' "$sdk_dir"
else
    printf '[missing] Android SDK   %s\n' "$sdk_dir"
    failed=1
fi

if [[ -x "$sdk_dir/platform-tools/adb" ]] || command -v adb >/dev/null 2>&1; then
    printf '[ok] adb                available\n'
else
    printf '[missing] adb           install Android SDK Platform-Tools\n'
    failed=1
fi

if [[ -x "$repo_dir/android/gradlew" ]]; then
    printf '[ok] Gradle wrapper     android/gradlew\n'
else
    printf '[missing] Gradle wrapper android/gradlew\n'
    failed=1
fi

for model in \
    models/alexa.onnx \
    models/hey_hyusk.onnx \
    models/hey_hyusk.onnx.data; do
    if [[ -f "$repo_dir/$model" ]]; then
        printf '[ok] Wake asset         %s\n' "$model"
    else
        printf '[optional] Wake asset   %s\n' "$model"
    fi
done

printf '\n'
if (( failed )); then
    printf 'Install Android Studio with SDK 36, Platform-Tools, and its bundled JDK 17.\n'
    printf 'Then set ANDROID_SDK_ROOT or create android/local.properties with sdk.dir=.\n'
    exit 1
fi

printf 'Environment is ready. Build with: cd android && ./gradlew assembleDebug\n'
