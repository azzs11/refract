# Pinned java.util.regex oracle. Build: docker build -t refract-oracle-java oracle/java
FROM eclipse-temurin:25-jdk-alpine@sha256:22edbb1021420ae6e248519d1af07577742314d67d4c60fcbb2e285b7e47fa78
WORKDIR /r
COPY Runner.java .
RUN javac -d /r Runner.java
ENTRYPOINT ["java", "-Xss16m", "-XX:+UseSerialGC", "-XX:TieredStopAtLevel=1", "-cp", "/r", "Runner"]
