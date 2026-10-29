# Project-specific ProGuard/R8 rules.

# WebRTC: native code calls back into Java by name (JNI), R8 must not rename or drop those classes.
-keep class org.webrtc.** { *; }
-dontwarn org.webrtc.**
