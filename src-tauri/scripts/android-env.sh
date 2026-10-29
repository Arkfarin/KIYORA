#!/usr/bin/env bash
# Source this file before invoking Android tools directly. Explicit environment
# variables take precedence over these Linux defaults.
export ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export NDK_HOME="${NDK_HOME:-$ANDROID_HOME/ndk/28.2.13676358}"
export ANDROID_NDK_HOME="$NDK_HOME"

if [[ -z "${JAVA_HOME:-}" ]]; then
  for kiyora_jdk in "$HOME/.local/share/kiyora/jdk-17" /usr/lib/jvm/java-17-openjdk /usr/lib/jvm/java-17-openjdk-amd64 /opt/android-studio/jbr; do
    if [[ -x "$kiyora_jdk/bin/javac" ]]; then
      export JAVA_HOME="$kiyora_jdk"
      break
    fi
  done
  unset kiyora_jdk
fi

export PATH="${JAVA_HOME:+$JAVA_HOME/bin:}$ANDROID_HOME/cmdline-tools/19.0/bin:$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/platform-tools:$PATH"
# TDLib and Rust can otherwise exhaust memory on a development laptop.
export KIYORA_BUILD_JOBS="${KIYORA_BUILD_JOBS:-2}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
