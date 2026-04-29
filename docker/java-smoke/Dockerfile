FROM maven:3.9-eclipse-temurin-21
WORKDIR /smoke
COPY pom.xml .
COPY entrypoint.sh .
COPY src ./src
ENTRYPOINT ["./entrypoint.sh"]
