#pragma once
#include "agent_types.hpp"
#include <QJsonArray>
#include <QSet>
namespace aigle {
inline QList<AgentProvider> agentProvidersFromJson(const QJsonArray& values) {
    QList<AgentProvider> result;
    QSet<QString> ids;
    for (const auto& value : values) {
        const auto object = value.toObject();
        AgentProvider provider{object["id"].toString(), object["name"].toString(), object["endpoint"].toString(), object["model"].toString(), {}};
        if (provider.id.isEmpty() || ids.contains(provider.id) || provider.endpoint.isEmpty()) continue;
        ids.insert(provider.id);
        for (const auto& model : object["models"].toArray())
            if (model.isString() && !model.toString().trimmed().isEmpty() && !provider.models.contains(model.toString())) provider.models.append(model.toString());
        result.append(provider);
    }
    return result;
}
inline QJsonArray toJson(const QList<AgentProvider>& providers) {
    QJsonArray result;
    for (const auto& provider : providers) result.append(QJsonObject{{"id", provider.id}, {"name", provider.name},
        {"endpoint", provider.endpoint}, {"model", provider.model}, {"models", QJsonArray::fromStringList(provider.models)}});
    return result;
}
inline QList<AgentMessage> agentMessagesFromJson(const QJsonArray& values) {
    QList<AgentMessage> result;
    for (const auto& value : values) {
        const auto object = value.toObject();
        result.append({object["role"].toString(), object["text"].toString(),
            QByteArray::fromBase64(object["image"].toString().toLatin1()), object["profile"].toObject()});
    }
    return result;
}
inline QJsonArray toJson(const QList<AgentMessage>& messages) {
    QJsonArray result;
    for (const auto& message : messages) {
        QJsonObject object{{"role", message.role}, {"text", message.text}};
        if (!message.image.isEmpty()) object["image"] = QString::fromLatin1(message.image.toBase64());
        if (!message.profile.isEmpty()) object["profile"] = message.profile;
        result.append(object);
    }
    return result;
}
inline AgentState agentStateFromJson(const QJsonObject& object) {
    AgentState result;
    result.running = object["running"].toBool();
    result.proposal = object["proposal"].toBool();
    if (object.contains("run_id")) result.runId = object["run_id"].toInteger();
    result.events = agentMessagesFromJson(object["events"].toArray());
    if (object.contains("error")) result.error = object["error"].toString();
    result.message = object["message"].toString();
    result.params = object["params"].toObject();
    const auto stats = object["stats"].toObject();
    if (!stats.isEmpty()) result.stats = AgentUsage{stats["requests"].toInt(), stats["seconds"].toDouble(),
        stats["input_tokens"].toInteger(), stats["output_tokens"].toInteger()};
    return result;
}
inline CredentialResult credentialResultFromJson(const QJsonObject& object) {
    CredentialResult result{object["request_id"].toString(), {}};
    if (object.contains("error")) result.error = object["error"].toString();
    return result;
}
inline ModelDiscovery modelDiscoveryFromJson(const QJsonObject& object) {
    ModelDiscovery result;
    result.requestId = object["request_id"].toString();
    result.running = object["running"].toBool();
    if (object.contains("error")) result.error = object["error"].toString();
    for (const auto& value : object["models"].toArray()) result.models.append(value.toString());
    return result;
}
}
