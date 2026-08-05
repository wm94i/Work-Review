use crate::analysis::report_blocks::{
    apply_preferences, assemble_with_section_count, default_local_order, format_section_heading,
};
use crate::analysis::{
    append_custom_prompt_for_locale, format_duration_for_locale, generate_activity_timeline,
    translate_semantic_category_name, Analyzer, AppLocale, GeneratedReport,
};
use crate::database::{Activity, DailyStats};
use crate::error::{AppError, Result};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use reqwest::Client;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

fn format_domain_label(
    domain: &crate::database::DomainUsage,
    locale: AppLocale,
    semantic_overrides: &HashMap<String, String>,
) -> String {
    match domain.semantic_category.as_deref().map(str::trim) {
        Some(semantic_category) if !semantic_category.is_empty() => {
            let semantic_category =
                translate_semantic_category_name(semantic_category, locale, semantic_overrides);
            match locale {
                AppLocale::En => format!("{} ({})", domain.domain, semantic_category),
                AppLocale::Ar => format!("{} ({})", domain.domain, semantic_category),
                _ => format!("{}（{}）", domain.domain, semantic_category),
            }
        }
        _ => domain.domain.clone(),
    }
}

fn empty_value(locale: AppLocale) -> &'static str {
    match locale {
        AppLocale::ZhCn => "无",
        AppLocale::ZhTw => "無",
        AppLocale::En => "None",
        AppLocale::Ar => "لا يوجد",
    }
}

fn join_list(locale: AppLocale, items: Vec<String>) -> String {
    items.join(if locale == AppLocale::En { ", " } else if locale == AppLocale::Ar { "، " } else { "、" })
}

/// 本地多模态分析器
/// 使用 Ollama 运行本地多模态模型（如 LLaVA）
pub struct LocalAnalyzer {
    host: String,
    model: String,
    custom_prompt: String,
    locale: AppLocale,
    pinned_blocks: Vec<String>,
    client: Client,
}

impl LocalAnalyzer {
    pub fn new(
        host: &str,
        model: &str,
        custom_prompt: &str,
        locale: AppLocale,
        pinned_blocks: Vec<String>,
        ai_request_timeout_secs: u64,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(ai_request_timeout_secs))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            host: host.to_string(),
            model: model.to_string(),
            custom_prompt: custom_prompt.to_string(),
            locale,
            pinned_blocks,
            client,
        }
    }

    fn generation_prompt(&self, date: &str, stats: &DailyStats, activities: &[Activity]) -> String {
        let apps_list = join_list(
            self.locale,
            stats
                .app_usage
                .iter()
                .take(5)
                .map(|app| {
                    format!(
                        "{}: {}",
                        app.app_name,
                        format_duration_for_locale(app.duration, self.locale)
                    )
                })
                .collect(),
        );

        let urls_list = join_list(
            self.locale,
            stats
                .domain_usage
                .iter()
                .take(3)
                .map(|domain| format_domain_label(domain, self.locale, &HashMap::new()))
                .collect(),
        );

        let keywords = activities
            .iter()
            .filter_map(|activity| activity.ocr_text.as_ref())
            .flat_map(|text| {
                text.split(|c: char| !c.is_alphanumeric() && c != '-')
                    .filter(|word| {
                        let len = word.chars().count();
                        (2..=20).contains(&len)
                    })
                    .take(3)
                    .map(|item| item.to_string())
            })
            .take(20)
            .collect::<Vec<_>>();

        let timeline = generate_activity_timeline(activities, self.locale);

        let base_prompt = match self.locale {
            AppLocale::ZhCn => format!(
                r#"你是一位风趣但可靠的工作效率分析师。请根据以下工作数据，生成一份自然、具体、有帮助的日报补充分析。

## 今日数据
- 日期：{date}
- 总工作时长：{}
- 使用的应用：{}
- 访问的网站：{}
- 从屏幕内容提取的关键词：{}

## 活动时间线
{timeline}

## 要求
请用简洁自然的中文输出以下三个部分：

## 今日工作内容
根据应用、网站和关键词，概括今天主要在推进什么工作，2 到 4 句话。

## 专注度分析
结合时长分布和应用切换，判断今天的专注状态，给出简短评价。

## 明日建议
给 1 条具体可执行的改进建议。

不要输出额外前言或解释。"#,
                format_duration_for_locale(stats.total_duration, self.locale),
                if apps_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    apps_list
                },
                if urls_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    urls_list
                },
                if keywords.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    join_list(self.locale, keywords)
                },
                timeline = timeline,
            ),
            AppLocale::ZhTw => format!(
                r#"你是一位可靠又自然的工作效率分析師。請根據以下工作資料，生成一份具體、有幫助的日報補充分析。

## 今日資料
- 日期：{date}
- 總工作時長：{}
- 使用的應用：{}
- 造訪的網站：{}
- 從畫面內容提取的關鍵詞：{}

## 活動時間線
{timeline}

## 要求
請用繁體中文輸出以下三個部分：

## 今日工作內容
根據應用、網站與關鍵詞，概括今天主要在推進什麼工作，2 到 4 句話。

## 專注度分析
結合時長分布與應用切換，判斷今天的專注狀態，給出簡短評價。

## 明日建議
給 1 條具體可執行的改進建議。

不要輸出額外前言或解釋。"#,
                format_duration_for_locale(stats.total_duration, self.locale),
                if apps_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    apps_list
                },
                if urls_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    urls_list
                },
                if keywords.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    join_list(self.locale, keywords)
                },
                timeline = timeline,
            ),
            AppLocale::En => format!(
                r#"You are a concise and reliable work-review assistant. Based on the data below, write a practical daily work analysis.

## Daily data
- Date: {date}
- Total work duration: {}
- Apps used: {}
- Websites visited: {}
- Keywords extracted from screen content: {}

## Activity Timeline
{timeline}

## Requirements
Write in English and output exactly these three sections:

## Today's work
Summarize what the user mainly worked on today in 2 to 4 sentences.

## Focus assessment
Comment briefly on focus and time allocation based on app usage and switching patterns.

## Next-step suggestion
Give 1 concrete and practical suggestion for tomorrow.

Do not add any extra preface or explanation."#,
                format_duration_for_locale(stats.total_duration, self.locale),
                if apps_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    apps_list
                },
                if urls_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    urls_list
                },
                if keywords.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    join_list(self.locale, keywords)
                },
                timeline = timeline,
            ),
            AppLocale::Ar => format!(
                r#"أنت مساعد تقييم عمل دقيق وموثوق. بناءً على البيانات أدناه، اكتب تحليلًا عمليًا لعمل اليوم.

## البيانات اليومية
- التاريخ: {date}
- إجمالي مدة العمل: {}
- التطبيقات المستخدمة: {}
- المواقع المزارة: {}
- الكلمات الرئيسية المستخرجة من محتوى الشاشة: {}

## الجدول الزمني للنشاط
{timeline}

## المتطلبات
يرجى الكتابة باللغة العربية وإخراج الأقسام الثلاثة التالية بدقة:

## عمل اليوم
لخص ما عمل عليه المستخدم اليوم في جملتين إلى 4 جمل.

## تقييم التركيز
علق بإيجاز على التركيز وتوزيع الوقت بناءً على استخدام التطبيقات وأنماط التبديل.

## اقتراح للخطوة التالية
قدم اقتراحًا واحدًا ملموسًا وعمليًا للغد.

لا تضف أي مقدمة أو شرح إضافي."#,
                format_duration_for_locale(stats.total_duration, self.locale),
                if apps_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    apps_list
                },
                if urls_list.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    urls_list
                },
                if keywords.is_empty() {
                    empty_value(self.locale).to_string()
                } else {
                    join_list(self.locale, keywords)
                },
                timeline = timeline,
            ),
        };

        append_custom_prompt_for_locale(base_prompt, &self.custom_prompt, self.locale)
    }

    async fn generate_with_ollama(
        &self,
        date: &str,
        stats: &DailyStats,
        activities: &[Activity],
    ) -> Result<String> {
        let prompt = self.generation_prompt(date, stats, activities);

        let response = self
            .client
            .post(format!("{}/api/generate", self.host))
            .json(&json!({
                "model": self.model,
                "prompt": prompt,
                "stream": false,
                "options": {
                    "temperature": 0.2,
                    "seed": 42
                }
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AppError::Analysis(format!(
                "Ollama API 错误: {}",
                response.status()
            )));
        }

        let result: serde_json::Value = response.json().await?;
        let ai_content = result["response"].as_str().unwrap_or("").trim().to_string();

        if ai_content.is_empty() {
            return Err(AppError::Analysis("Ollama 返回空内容".to_string()));
        }

        Ok(ai_content)
    }

    #[allow(dead_code)]
    async fn analyze_screenshot(&self, screenshot_path: &Path) -> Result<String> {
        let image_data = tokio::fs::read(screenshot_path).await?;
        let image_base64 = BASE64_STANDARD.encode(&image_data);

        let screenshot_prompt = match self.locale {
            AppLocale::ZhCn => "请简要描述这张截图里的工作内容，用中文回答，限制在 50 字以内。",
            AppLocale::ZhTw => "請簡要描述這張截圖裡的工作內容，請用繁體中文回答，限制在 50 字內。",
            AppLocale::En => {
                "Briefly describe what work is shown in this screenshot in under 50 words."
            }
            AppLocale::Ar => {
                "صف بإيجاز العمل الموضح في هذه اللقطة في أقل من 50 كلمة."
            }
        };

        let response = self
            .client
            .post(format!("{}/api/generate", self.host))
            .json(&json!({
                "model": self.model,
                "prompt": screenshot_prompt,
                "images": [image_base64],
                "stream": false,
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(AppError::Analysis(format!(
                "Ollama API 错误: {}",
                response.status()
            )));
        }

        let result: serde_json::Value = response.json().await?;
        Ok(result["response"].as_str().unwrap_or("").to_string())
    }
}

#[async_trait]
impl Analyzer for LocalAnalyzer {
    async fn generate_report(
        &self,
        date: &str,
        stats: &DailyStats,
        activities: &[Activity],
        _screenshots_dir: &Path,
        locale: AppLocale,
        category_name_overrides: HashMap<String, String>,
        semantic_name_overrides: HashMap<String, String>,
    ) -> Result<GeneratedReport> {
        log::info!("生成本地日报（尝试调用 Ollama）");

        let mut report = match locale {
            AppLocale::ZhCn => format!("# 工作日报 - {date}\n\n"),
            AppLocale::ZhTw => format!("# 工作日報 - {date}\n\n"),
            AppLocale::En => format!("# Daily Report - {date}\n\n"),
            AppLocale::Ar => format!("# تقرير اليوم - {date}\n\n"),
        };
        let mut used_ai = false;
        let mut fallback_reason = None;

        let (stats_sections, mut section_count) = assemble_with_section_count(
            &apply_preferences(default_local_order(), &self.pinned_blocks),
            stats,
            locale,
            &category_name_overrides,
            &semantic_name_overrides,
        );
        report.push_str(&stats_sections);

        match self.generate_with_ollama(date, stats, activities).await {
            Ok(ai_content) => {
                report.push('\n');
                report.push_str(&ai_content);
                used_ai = true;
            }
            Err(error) => {
                log::warn!("Ollama 调用失败，使用备选内容: {error}");
                fallback_reason = Some(match locale {
                    AppLocale::ZhCn => "请求失败，已回退到基础模板".to_string(),
                    AppLocale::ZhTw => "請求失敗，已回退到基礎模板".to_string(),
                    AppLocale::En => {
                        "the AI request failed, so the report fell back to the base template"
                            .to_string()
                    }
                    AppLocale::Ar => {
                        "فشل طلب الذكاء الاصطناعي، لذا رجع التقرير إلى القالب الأساسي"
                            .to_string()
                    }
                });
                let apps_list = join_list(
                    locale,
                    stats
                        .app_usage
                        .iter()
                        .take(3)
                        .map(|app| app.app_name.clone())
                        .collect(),
                );

                match locale {
                    AppLocale::ZhCn => {
                        section_count += 1;
                        report.push('\n');
                        report.push_str(&format_section_heading(
                            locale,
                            section_count,
                            "今日工作内容",
                        ));
                        report.push_str(&format!(
                            "今日主要使用 {} 等应用进行工作，整体推进比较稳定。\n",
                            if apps_list.is_empty() {
                                "多个".to_string()
                            } else {
                                apps_list
                            }
                        ));
                        section_count += 1;
                        report.push('\n');
                        report.push_str(&format_section_heading(
                            locale,
                            section_count,
                            "专注度分析",
                        ));
                        report.push_str("今天整体节奏较稳定，可以继续保持当前的工作推进方式。\n");
                        section_count += 1;
                        report.push('\n');
                        report.push_str(&format_section_heading(locale, section_count, "明日建议"));
                        report
                            .push_str("建议为核心任务预留更完整的连续时间段，减少被打断的次数。\n");
                        report.push_str("\n---\n*注：AI 分析暂不可用，使用基础模板生成。*");
                    }
                    AppLocale::ZhTw => {
                        section_count += 1;
                        report.push('\n');
                        report.push_str(&format_section_heading(
                            locale,
                            section_count,
                            "今日工作內容",
                        ));
                        report.push_str(&format!(
                            "今日主要使用 {} 等應用進行工作，整體推進相對穩定。\n",
                            if apps_list.is_empty() {
                                "多個".to_string()
                            } else {
                                apps_list
                            }
                        ));
                        section_count += 1;
                        report.push('\n');
                        report.push_str(&format_section_heading(
                            locale,
                            section_count,
                            "專注度分析",
                        ));
                        report.push_str("今天整體節奏較穩定，可以延續目前的工作推進方式。\n");
                        section_count += 1;
                        report.push('\n');
                        report.push_str(&format_section_heading(locale, section_count, "明日建議"));
                        report
                            .push_str("建議為核心任務預留更完整的連續時間段，減少被打斷的次數。\n");
                        report.push_str("\n---\n*註：AI 分析暫時不可用，目前使用基礎模板生成。*");
                    }
                    AppLocale::En => {
                        section_count += 1;
                        report.push('\n');
                        report.push_str(&format_section_heading(
                            locale,
                            section_count,
                            "Today's work",
                        ));
                        report.push_str(&format!(
                            "Today mainly involved work across {} and related tools, with a fairly steady pace overall.\n",
                            if apps_list.is_empty() {
                                "several apps".to_string()
                            } else {
                                apps_list
                            }
                        ));
                        section_count += 1;
                        report.push('\n');
                        report.push_str(&format_section_heading(
                            locale,
                            section_count,
                            "Focus assessment",
                        ));
                        report.push_str("The overall rhythm looks stable today, and the workflow stayed reasonably consistent.\n");
                        section_count += 1;
                        report.push('\n');
                        report.push_str(&format_section_heading(
                            locale,
                            section_count,
                            "Next-step suggestion",
                        ));
                        report.push_str("Reserve a longer uninterrupted block for the main task tomorrow to reduce context switching.\n");
                        report.push_str("\n---\n*Note: AI analysis is currently unavailable, so this report was generated from the base template.*");
                    }
                    AppLocale::Ar => {
                        section_count += 1;
                        report.push('\n');
                        report.push_str(&format_section_heading(
                            locale,
                            section_count,
                            "عمل اليوم",
                        ));
                        report.push_str(&format!(
                            "شمل العمل اليوم بشكل أساسي {} والأدوات ذات الصلة، مع وتيرة ثابتة بشكل عام.\n",
                            if apps_list.is_empty() {
                                "عدة تطبيقات".to_string()
                            } else {
                                apps_list
                            }
                        ));
                        section_count += 1;
                        report.push('\n');
                        report.push_str(&format_section_heading(
                            locale,
                            section_count,
                            "تقييم التركيز",
                        ));
                        report.push_str("يبدو الإيقاع العام مستقراً اليوم، وظل سير العمل متسقاً بشكل معقول.\n");
                        section_count += 1;
                        report.push('\n');
                        report.push_str(&format_section_heading(
                            locale,
                            section_count,
                            "اقتراح للخطوة التالية",
                        ));
                        report.push_str("خصص فترات زمنية غير متقطعة أطول للمهمة الرئيسية غداً لتقليل التبديل بين المهام.\n");
                        report.push_str("\n---\n*ملاحظة: تحليل الذكاء الاصطناعي غير متوفر حالياً، لذا تم إنشاء هذا التقرير باستخدام القالب الأساسي.*");
                    }
                }
            }
        }

        // 活动时间线放在最底部
        let timeline = generate_activity_timeline(activities, locale);
        if !timeline.is_empty() {
            report.push_str("\n\n");
            report.push_str(&timeline);
        }

        Ok(GeneratedReport {
            content: report,
            used_ai,
            fallback_reason,
            ai_order: None,
        })
    }
}
