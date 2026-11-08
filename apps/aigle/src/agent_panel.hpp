#pragma once
#include "agent_types.hpp"
#include "commands.hpp"
#include <QImage>
#include <QComboBox>
#include <QHash>
#include <QSet>
#include <QJsonObject>
#include <QJsonArray>
#include <QLabel>
#include <QLineEdit>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QTextBrowser>
#include <QToolButton>
#include <QWidget>

namespace aigle {
class AgentPanel final : public QWidget {
    Q_OBJECT
  public:
    explicit AgentPanel(QWidget* parent = nullptr);
    void setPreview(const QImage& image);
    void setPhoto(const QString& path, const QString& copy = {}, const QString& identity = {});
    void setState(const AgentState& state, bool photoOpen);
  signals:
    void command(const aigle::EngineCommand& request);
    void credentialFinished(const aigle::CredentialResult& result);
    void modelsFinished(const aigle::ModelDiscovery& result);

  private:
    QLineEdit *endpoint_, *model_, *key_;
    QPlainTextEdit* prompt_;
    QTextBrowser* transcript_;
    QWidget* proposal_;
    QLabel *preview_, *summary_, *context_;
    QToolButton* modelButton_;
    QString photo_, conversationKey_, providerId_;
    QComboBox* providerChoice_;
    QList<AgentProvider> providers_;
    QHash<QString, QString> sessionKeys_;
    void refreshProviders();
    void activateProvider(const QString& id);
    void persistProviders();
    QList<AgentMessage> messages_;
    void appendMessage(const QString& role, const QString& text);
    void renderConversation();
    void renderSummary();
    QJsonObject summaryParams_;
    std::optional<AgentUsage> usageStats_;
    bool conversationSaveFailed_ = false;
    void persistConversation();
    qint64 runId_ = -1;
    int receivedEvents_ = 0;
    QSet<int> expandedProfiles_;
    QString conversationPrompt() const;
    bool eventFilter(QObject* watched, QEvent* event) override;
    QPushButton *send_, *accept_, *reject_, *cancel_;
    QLabel* usage_;
    QString lastMessage_;
};
} // namespace aigle
