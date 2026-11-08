#pragma once
#include <QByteArray>
#include <QJsonObject>
#include <QList>
#include <QMetaType>
#include <QStringList>
#include <optional>

namespace aigle {
struct AgentProvider { QString id, name, endpoint, model; QStringList models; };
struct AgentMessage {
    QString role, text;
    QByteArray image;
    // A profile is a schema-defined map of adjustment paths to values.
    QJsonObject profile;
};
struct AgentUsage {
    int requests = 0;
    double seconds = 0;
    qint64 inputTokens = 0, outputTokens = 0;
};
struct AgentState {
    bool running = false, proposal = false;
    std::optional<qint64> runId;
    QList<AgentMessage> events;
    std::optional<QString> error;
    QString message;
    QJsonObject params;
    std::optional<AgentUsage> stats;
};
struct CredentialResult {
    QString requestId;
    std::optional<QString> error;
};
struct ModelDiscovery {
    QString requestId;
    bool running = false;
    std::optional<QString> error;
    QStringList models;
};
}
Q_DECLARE_METATYPE(aigle::CredentialResult)
Q_DECLARE_METATYPE(aigle::ModelDiscovery)
