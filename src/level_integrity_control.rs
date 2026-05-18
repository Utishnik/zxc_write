/*  Возможно потом уберется от сюда
fn lower_thread_to_low_il() -> Result<()> {
    let mut token = HANDLE::default();

    unsafe {
        // Пробуем открыть токен потока
        let open_result = OpenThreadToken(
            GetCurrentThread(),
            TOKEN_QUERY | TOKEN_ADJUST_DEFAULT,
            false,
            &mut token,
        );

        if open_result.is_err() {
            // Нет токена потока — дублируем из процесса
            let mut proc_token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE, &mut proc_token)?;

            let mut imp_token = HANDLE::default();
            DuplicateTokenEx(
                proc_token,
                TOKEN_QUERY | TOKEN_ADJUST_DEFAULT,
                None,
                SecurityImpersonation,
                TokenImpersonation,
                &mut imp_token,
            )?;

            CloseHandle(proc_token)?;

            SetThreadToken(Some(GetCurrentThread()), imp_token)?;
            // imp_token теперь принадлежит потоку

            // Повторно открываем токен потока
            OpenThreadToken(
                GetCurrentThread(),
                TOKEN_QUERY | TOKEN_ADJUST_DEFAULT,
                false,
                &mut token,
            )?;
        }
    }

    // Создаём SID для Low Integrity (S-1-16-4096)
    let mut sid: PSID = std::ptr::null_mut();
    unsafe {
        AllocateAndInitializeSid(
            &SID_IDENTIFIER_AUTHORITY { Value: [0, 0, 0, 0, 0, 16] },
            1,
            4096, // SECURITY_MANDATORY_LOW_RID
            0, 0, 0, 0, 0, 0,
            &mut sid,
        )?;
    }

    let label = TOKEN_MANDATORY_LABEL {
        Label: SID_AND_ATTRIBUTES {
            Sid: sid,
            Attributes: 0,
        },
    };

    unsafe {
        SetTokenInformation(
            token,
            windows::Win32::Security::TokenIntegrityLevel,
            &label as *const _ as *const _,
            std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32,
        )?;
    }

    unsafe { FreeSid(sid); }

    println!("Поток понижен до Low Integrity");
    Ok(())
}
*/
